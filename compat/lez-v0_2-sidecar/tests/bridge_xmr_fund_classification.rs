#![cfg(target_os = "linux")]

use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use borsh::to_vec;
use indexer_service_protocol::{
    Account as IndexedAccount, AccountId as IndexedAccountId, BedrockStatus, Block, BlockBody,
    BlockHeader, Data as IndexedData, HashType, ProgramId as IndexedProgramId,
    Proof as IndexedProof, PublicKey as IndexedPublicKey, PublicMessage as IndexedPublicMessage,
    PublicTransaction as IndexedPublicTransaction, Signature as IndexedSignature, Transaction,
    WitnessSet as IndexedWitnessSet,
};
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    ClassifyFinalizedNativeXmrEffectV3Request, DiscoveryWindow, ErrorCode,
    FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    FinalizedNativeXmrUnavailableReasonV3, Hex32, MessageContext, Participant,
    PrepareNativeXmrEscrowV3Request, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    XmrNativeEffectV3, XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3,
    XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
    prepared_from_transaction, program_id_to_hex, start_bridge_server,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, Signature, public_transaction::WitnessSet,
};
use tempfile::TempDir;

const CAPABILITY: &str = "xmr-fund-classifier-capability-0001";
const RUN_ID: &str = "xmr-fund-classifier-run";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [51; 32];
const FUNDING_BLOCK: u64 = 10;
const FINALIZED_END: u64 = 11;

#[derive(Debug)]
struct CountingNonce {
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for CountingNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
    }
}

#[derive(Debug)]
struct FixtureIndexer {
    blocks: BTreeMap<u64, Block>,
    by_hash: BTreeMap<[u8; 32], Block>,
    accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl FinalizedIndexerApi for FixtureIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push("tip".to_owned());
        Ok(Some(FINALIZED_END))
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("id:{block_id}"));
        Ok(self.blocks.get(&block_id).cloned())
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("hash:{}", hex::encode(block_hash)));
        Ok(self.by_hash.get(&block_hash).cloned())
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("account:{}:{block_id}", hex::encode(account_id)));
        self.accounts
            .get(&(account_id, block_id))
            .cloned()
            .ok_or(BridgeRuntimeError::Unavailable)
    }
}

#[derive(Clone, Copy, Debug)]
enum UnavailableStage {
    Finality,
    History,
}

#[derive(Debug)]
struct UnavailableEvidenceIndexer {
    stage: UnavailableStage,
}

#[async_trait]
impl FinalizedIndexerApi for UnavailableEvidenceIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        Ok(match self.stage {
            UnavailableStage::Finality => None,
            UnavailableStage::History => Some(FINALIZED_END),
        })
    }

    async fn block_by_id(&self, _block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok(None)
    }

    async fn block_by_hash(
        &self,
        _block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok(None)
    }

    async fn account_at_block(
        &self,
        _account_id: [u8; 32],
        _block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }
}

#[derive(Debug)]
struct MovingEndIndexer {
    base: Arc<FixtureIndexer>,
    changed_end: Block,
    end_reads: AtomicUsize,
}

#[async_trait]
impl FinalizedIndexerApi for MovingEndIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        Ok(Some(FINALIZED_END))
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        if block_id == FINALIZED_END && self.end_reads.fetch_add(1, Ordering::SeqCst) >= 2 {
            return Ok(Some(self.changed_end.clone()));
        }
        self.base.block_by_id(block_id).await
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        if block_hash == self.changed_end.header.hash.0 {
            return Ok(Some(self.changed_end.clone()));
        }
        self.base.block_by_hash(block_hash).await
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        self.base.account_at_block(account_id, block_id).await
    }
}

#[derive(Debug)]
struct RegressingFinalityIndexer {
    base: Arc<FixtureIndexer>,
    tip_reads: AtomicUsize,
}

#[async_trait]
impl FinalizedIndexerApi for RegressingFinalityIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        if self.tip_reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Some(FINALIZED_END))
        } else {
            Ok(Some(FUNDING_BLOCK - 1))
        }
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        self.base.block_by_id(block_id).await
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        self.base.block_by_hash(block_hash).await
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        self.base.account_at_block(account_id, block_id).await
    }
}

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("valid private key");
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key, public)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn runtime_for(role: Participant, signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        program_id_to_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn runtime(signer: AccountId) -> RuntimeDescriptor {
    runtime_for(Participant::Taker, signer)
}

fn terms(depositor: AccountId, claimant: AccountId) -> XmrNativeEscrowTermsV3 {
    let (claim_authority, _, claim_key) = account(23);
    let (refund_authority, _, refund_key) = account(24);
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(SWAP_ID),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(
            compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID).into_value(),
        ),
        custody_account_id: Hex32::from_bytes(
            compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID).into_value(),
        ),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_key.value()),
        claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_key.value()),
        refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: h(16),
        amount: 75,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: h(17),
        refund_message_hash: h(18),
        punish_message_hash: h(19),
    })
    .expect("valid XMR terms")
}

fn prepare_request(
    descriptor: RuntimeDescriptor,
    xmr_terms: &XmrNativeEscrowTermsV3,
) -> PrepareNativeXmrEscrowV3Request {
    PrepareNativeXmrEscrowV3Request::new(
        MessageContext::new(
            RunId::new(RUN_ID).expect("run id"),
            RequestId::new("xmr-fund-prepare").expect("request id"),
            Participant::Taker,
        ),
        descriptor,
        *xmr_terms,
    )
}

fn classification_request(
    descriptor: &RuntimeDescriptor,
    xmr_terms: &XmrNativeEscrowTermsV3,
    transaction: &lez_bridge_protocol::PreparedTransaction,
    request_id: &str,
) -> ClassifyFinalizedNativeXmrEffectV3Request {
    classification_request_for_effect(
        descriptor,
        xmr_terms,
        XmrNativeEffectV3::Fund,
        transaction,
        request_id,
    )
}

fn classification_request_for_effect(
    descriptor: &RuntimeDescriptor,
    xmr_terms: &XmrNativeEscrowTermsV3,
    effect: XmrNativeEffectV3,
    transaction: &lez_bridge_protocol::PreparedTransaction,
    request_id: &str,
) -> ClassifyFinalizedNativeXmrEffectV3Request {
    ClassifyFinalizedNativeXmrEffectV3Request::new(
        MessageContext::new(
            RunId::new(RUN_ID).expect("run id"),
            RequestId::new(request_id).expect("request id"),
            descriptor.sidecar_role,
        ),
        descriptor.clone(),
        *xmr_terms,
        effect,
        FinalizedNativeXmrTransactionTargetV3::exact(transaction.clone()),
        DiscoveryWindow::new(FUNDING_BLOCK, 2).expect("window"),
    )
}

fn indexed_public(public: &nssa::PublicTransaction) -> IndexedPublicTransaction {
    IndexedPublicTransaction {
        hash: HashType(public.hash()),
        message: IndexedPublicMessage {
            program_id: IndexedProgramId(public.message().program_id),
            account_ids: public
                .message()
                .account_ids
                .iter()
                .map(|account| IndexedAccountId {
                    value: account.into_value(),
                })
                .collect(),
            nonces: public
                .message()
                .nonces
                .iter()
                .map(|nonce| u128::from(*nonce))
                .collect(),
            instruction_data: public.message().instruction_data.clone(),
        },
        witness_set: IndexedWitnessSet {
            signatures_and_public_keys: public
                .witness_set()
                .signatures_and_public_keys()
                .iter()
                .map(|(signature, key)| {
                    (
                        IndexedSignature(signature.value),
                        IndexedPublicKey(*key.value()),
                    )
                })
                .collect(),
            proof: None,
        },
    }
}

fn finalized_block(block_id: u64, transactions: Vec<Transaction>) -> Block {
    let byte = u8::try_from(block_id).expect("fixture block ID");
    let previous = u8::try_from(block_id.saturating_sub(1)).expect("fixture previous ID");
    Block {
        header: BlockHeader {
            block_id,
            prev_block_hash: HashType([previous; 32]),
            hash: HashType([byte; 32]),
            timestamp: 1_000 + block_id,
            signature: IndexedSignature([byte; 64]),
        },
        body: BlockBody { transactions },
        bedrock_status: BedrockStatus::Finalized,
    }
}

fn metadata(xmr_terms: &XmrNativeEscrowTermsV3, status: EscrowStatus) -> EscrowMetadata {
    let input = xmr_terms.to_input();
    EscrowMetadata {
        version: 3,
        swap_id: *input.swap_id.as_bytes(),
        terms_hash: *input.activation_commitment.as_bytes(),
        claim_authority: ClaimAuthority::XmrDualAdaptor {
            claim_aggregate_x_only_public_key: *input.claim_aggregate_x_only_public_key.as_bytes(),
            claim_aggregate_account_id: AccountId::new(
                *input.claim_authority_account_id.as_bytes(),
            ),
            refund_aggregate_x_only_public_key: *input
                .refund_aggregate_x_only_public_key
                .as_bytes(),
            refund_aggregate_account_id: AccountId::new(
                *input.refund_authority_account_id.as_bytes(),
            ),
            maker_dleq_transcript_commitment: *input.maker_dleq_transcript_commitment.as_bytes(),
            taker_dleq_transcript_commitment: *input.taker_dleq_transcript_commitment.as_bytes(),
            claim_partial_context_binding: *input.claim_partial_context_binding.as_bytes(),
            claim_partial_commitment: *input.claim_partial_commitment.as_bytes(),
            punish_at: input.punish_at_ms,
        },
        depositor: AccountId::new(*input.depositor_account_id.as_bytes()),
        depositor_asset: AccountId::new(*input.depositor_account_id.as_bytes()),
        claimant: AccountId::new(*input.claimant_account_id.as_bytes()),
        claimant_asset: AccountId::new(*input.claimant_account_id.as_bytes()),
        custody: AccountId::new(*input.custody_account_id.as_bytes()),
        asset_program: TRANSFER_PROGRAM,
        custody_program: TRANSFER_PROGRAM,
        asset_definition: [0; 32],
        amount: input.amount,
        refund_at: input.refund_at_ms,
        status,
    }
}

fn finalized_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    let funding = decode_prepared_for_signer(&prepared.funding, depositor).expect("exact funding");
    let funding_block = finalized_block(
        FUNDING_BLOCK,
        vec![Transaction::Public(indexed_public(&funding))],
    );
    let end_block = finalized_block(FINALIZED_END, Vec::new());
    let input = xmr_terms.to_input();
    let accounts = BTreeMap::from([
        (
            (*input.metadata_account_id.as_bytes(), FUNDING_BLOCK),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(ESCROW_PROGRAM),
                balance: 0,
                data: IndexedData(
                    to_vec(&metadata(xmr_terms, EscrowStatus::Funded)).expect("metadata encoding"),
                ),
                nonce: 0,
            }),
        ),
        (
            (*input.custody_account_id.as_bytes(), FUNDING_BLOCK),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(TRANSFER_PROGRAM),
                balance: input.amount,
                data: IndexedData(Vec::new()),
                nonce: 0,
            }),
        ),
    ]);
    Arc::new(FixtureIndexer {
        blocks: BTreeMap::from([
            (FUNDING_BLOCK, funding_block.clone()),
            (FINALIZED_END, end_block.clone()),
        ]),
        by_hash: BTreeMap::from([
            (funding_block.header.hash.0, funding_block),
            (end_block.header.hash.0, end_block),
        ]),
        accounts,
        calls: Mutex::new(Vec::new()),
    })
}

fn finalized_initialization_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    let initialization = decode_prepared_for_signer(&prepared.initialization, depositor)
        .expect("exact initialization");
    let initialization_block = finalized_block(
        FUNDING_BLOCK,
        vec![Transaction::Public(indexed_public(&initialization))],
    );
    let end_block = finalized_block(FINALIZED_END, Vec::new());
    let input = xmr_terms.to_input();
    let accounts = BTreeMap::from([
        (
            (*input.metadata_account_id.as_bytes(), FUNDING_BLOCK),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(ESCROW_PROGRAM),
                balance: 0,
                data: IndexedData(
                    to_vec(&metadata(xmr_terms, EscrowStatus::Empty)).expect("metadata encoding"),
                ),
                nonce: 0,
            }),
        ),
        (
            (*input.custody_account_id.as_bytes(), FUNDING_BLOCK),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(TRANSFER_PROGRAM),
                balance: 0,
                data: IndexedData(Vec::new()),
                nonce: 0,
            }),
        ),
    ]);
    Arc::new(FixtureIndexer {
        blocks: BTreeMap::from([
            (FUNDING_BLOCK, initialization_block.clone()),
            (FINALIZED_END, end_block.clone()),
        ]),
        by_hash: BTreeMap::from([
            (initialization_block.header.hash.0, initialization_block),
            (end_block.header.hash.0, end_block),
        ]),
        accounts,
        calls: Mutex::new(Vec::new()),
    })
}

fn modified_initialization_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
    mutate: impl FnOnce(&mut Block),
) -> Arc<FixtureIndexer> {
    let mut indexer = Arc::into_inner(finalized_initialization_indexer(
        prepared, xmr_terms, depositor,
    ))
    .expect("fixture has one owner");
    let block = indexer
        .blocks
        .get_mut(&FUNDING_BLOCK)
        .expect("initialization block");
    mutate(block);
    indexer.by_hash.insert(block.header.hash.0, block.clone());
    Arc::new(indexer)
}

fn malformed_initialization_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    modified_initialization_indexer(prepared, xmr_terms, depositor, |block| {
        let Some(Transaction::Public(indexed)) = block.body.transactions.first_mut() else {
            panic!("public initialization transaction")
        };
        indexed.message.account_ids.swap(0, 1);
    })
}

fn proof_bearing_initialization_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    modified_initialization_indexer(prepared, xmr_terms, depositor, |block| {
        let Some(Transaction::Public(indexed)) = block.body.transactions.first_mut() else {
            panic!("public initialization transaction")
        };
        indexed.witness_set.proof = Some(IndexedProof(vec![13]));
    })
}

fn initialization_state_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
    status: EscrowStatus,
    custody_balance: u128,
) -> Arc<FixtureIndexer> {
    let mut indexer = Arc::into_inner(finalized_initialization_indexer(
        prepared, xmr_terms, depositor,
    ))
    .expect("fixture has one owner");
    let input = xmr_terms.to_input();
    indexer.accounts.insert(
        (*input.metadata_account_id.as_bytes(), FUNDING_BLOCK),
        HistoricalAccount::Present(IndexedAccount {
            program_owner: IndexedProgramId(ESCROW_PROGRAM),
            balance: 0,
            data: IndexedData(to_vec(&metadata(xmr_terms, status)).expect("metadata encoding")),
            nonce: 0,
        }),
    );
    indexer.accounts.insert(
        (*input.custody_account_id.as_bytes(), FUNDING_BLOCK),
        HistoricalAccount::Present(IndexedAccount {
            program_owner: IndexedProgramId(TRANSFER_PROGRAM),
            balance: custody_balance,
            data: IndexedData(Vec::new()),
            nonce: 0,
        }),
    );
    Arc::new(indexer)
}

fn missing_fund_indexer(
    xmr_terms: &XmrNativeEscrowTermsV3,
    status: EscrowStatus,
) -> Arc<FixtureIndexer> {
    let start_block = finalized_block(FUNDING_BLOCK, Vec::new());
    let end_block = finalized_block(FINALIZED_END, Vec::new());
    let input = xmr_terms.to_input();
    let balance = if status == EscrowStatus::Empty {
        0
    } else {
        input.amount
    };
    let accounts = BTreeMap::from([
        (
            (*input.metadata_account_id.as_bytes(), FINALIZED_END),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(ESCROW_PROGRAM),
                balance: 0,
                data: IndexedData(to_vec(&metadata(xmr_terms, status)).expect("metadata encoding")),
                nonce: 0,
            }),
        ),
        (
            (*input.custody_account_id.as_bytes(), FINALIZED_END),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(TRANSFER_PROGRAM),
                balance,
                data: IndexedData(Vec::new()),
                nonce: 0,
            }),
        ),
    ]);
    Arc::new(FixtureIndexer {
        blocks: BTreeMap::from([
            (FUNDING_BLOCK, start_block.clone()),
            (FINALIZED_END, end_block.clone()),
        ]),
        by_hash: BTreeMap::from([
            (start_block.header.hash.0, start_block),
            (end_block.header.hash.0, end_block),
        ]),
        accounts,
        calls: Mutex::new(Vec::new()),
    })
}

fn modified_funding_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
    mutate: impl FnOnce(&mut Block),
) -> Arc<FixtureIndexer> {
    let mut indexer = Arc::into_inner(finalized_indexer(prepared, xmr_terms, depositor))
        .expect("fixture has one owner");
    let block = indexer
        .blocks
        .get_mut(&FUNDING_BLOCK)
        .expect("funding block");
    mutate(block);
    indexer.by_hash.insert(block.header.hash.0, block.clone());
    Arc::new(indexer)
}

fn conflicting_fund_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    modified_funding_indexer(prepared, xmr_terms, depositor, |block| {
        let duplicate = block
            .body
            .transactions
            .first()
            .expect("funding transaction")
            .clone();
        block.body.transactions.push(duplicate);
    })
}

fn malformed_same_target_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    modified_funding_indexer(prepared, xmr_terms, depositor, |block| {
        let Some(Transaction::Public(indexed)) = block.body.transactions.first_mut() else {
            panic!("public funding transaction")
        };
        indexed.message.account_ids.swap(0, 1);
    })
}

fn proof_bearing_fund_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<FixtureIndexer> {
    modified_funding_indexer(prepared, xmr_terms, depositor, |block| {
        let Some(Transaction::Public(indexed)) = block.body.transactions.first_mut() else {
            panic!("public funding transaction")
        };
        indexed.witness_set.proof = Some(IndexedProof(vec![7]));
    })
}

fn absent_state_indexer(xmr_terms: &XmrNativeEscrowTermsV3) -> Arc<FixtureIndexer> {
    let mut indexer = Arc::into_inner(missing_fund_indexer(xmr_terms, EscrowStatus::Empty))
        .expect("fixture has one owner");
    let input = xmr_terms.to_input();
    indexer.accounts.insert(
        (*input.metadata_account_id.as_bytes(), FINALIZED_END),
        HistoricalAccount::Absent,
    );
    indexer.accounts.insert(
        (*input.custody_account_id.as_bytes(), FINALIZED_END),
        HistoricalAccount::Absent,
    );
    Arc::new(indexer)
}

fn one_sided_missing_indexer(xmr_terms: &XmrNativeEscrowTermsV3) -> Arc<FixtureIndexer> {
    let mut indexer = Arc::into_inner(missing_fund_indexer(xmr_terms, EscrowStatus::Empty))
        .expect("fixture has one owner");
    indexer.accounts.insert(
        (
            *xmr_terms.to_input().custody_account_id.as_bytes(),
            FINALIZED_END,
        ),
        HistoricalAccount::Absent,
    );
    Arc::new(indexer)
}

fn regressing_finality_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<RegressingFinalityIndexer> {
    Arc::new(RegressingFinalityIndexer {
        base: finalized_indexer(prepared, xmr_terms, depositor),
        tip_reads: AtomicUsize::new(0),
    })
}

fn moving_end_indexer(xmr_terms: &XmrNativeEscrowTermsV3) -> Arc<MovingEndIndexer> {
    let base = missing_fund_indexer(xmr_terms, EscrowStatus::Empty);
    let mut changed_end = finalized_block(FINALIZED_END, Vec::new());
    changed_end.header.hash = HashType([99; 32]);
    changed_end.header.signature = IndexedSignature([99; 64]);
    Arc::new(MovingEndIndexer {
        base,
        changed_end,
        end_reads: AtomicUsize::new(0),
    })
}

fn moving_initialization_end_indexer(
    prepared: &lez_bridge_protocol::PrepareNativeXmrEscrowV3Result,
    xmr_terms: &XmrNativeEscrowTermsV3,
    depositor: AccountId,
) -> Arc<MovingEndIndexer> {
    let base = finalized_initialization_indexer(prepared, xmr_terms, depositor);
    let mut changed_end = finalized_block(FINALIZED_END, Vec::new());
    changed_end.header.hash = HashType([98; 32]);
    changed_end.header.signature = IndexedSignature([98; 64]);
    Arc::new(MovingEndIndexer {
        base,
        changed_end,
        end_reads: AtomicUsize::new(0),
    })
}

async fn start_node() -> (String, jsonrpsee::server::ServerHandle, Arc<AtomicUsize>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("node binds");
    let address = server.local_addr().expect("node address");
    let mut rpc = RpcModule::new(Arc::clone(&sends));
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .expect("health method");
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([41_u8; 32]))
    })
    .expect("channel method");
    rpc.register_method("sendTransaction", |_, sends, _| {
        sends.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(hex::encode([42_u8; 32]))
    })
    .expect("send method");
    (format!("http://{address}"), server.start(rpc), sends)
}

async fn start_classifier_sidecar(
    state_root: &Path,
    idempotency_name: &str,
    descriptor: RuntimeDescriptor,
    planner: Arc<NativeEscrowPlanner>,
    node_endpoint: &str,
    indexer: Arc<dyn FinalizedIndexerApi>,
) -> (BridgeClient, lez_v0_2_sidecar::BridgeServerHandle) {
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        planner,
        Arc::new(OfficialNodeRpc::connect(node_endpoint).expect("official node")),
        indexer,
    ));
    let bridge = start_bridge_server(
        BridgeServerConfig::new(
            RunId::new(RUN_ID).expect("run id"),
            BridgeServerCapability::new(CAPABILITY).expect("server capability"),
            state_root.join(idempotency_name),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .expect("sidecar starts");
    let client = BridgeClient::connect(BridgeClientConfig::new(
        bridge.endpoint(),
        SidecarCapability::new(CAPABILITY).expect("client capability"),
        RunId::new(RUN_ID).expect("run id"),
        descriptor,
        Duration::from_secs(2),
    ))
    .expect("client");
    (client, bridge)
}

fn assert_remote_code(error: BridgeClientError, expected: ErrorCode) {
    let BridgeClientError::Remote(remote) = error else {
        panic!("expected typed remote error, got {error:?}")
    };
    assert_eq!(remote.code(), expected);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one actor-authenticated journey keeps durable Found and fail-closed classifications joined"
)]
async fn authenticated_exact_persisted_fund_requires_stable_finalized_history() {
    let directory = TempDir::new().expect("state root");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).expect("private root");
    let planner_directory = directory.path().join("planner");
    fs::create_dir(&planner_directory).expect("planner directory");
    fs::set_permissions(&planner_directory, fs::Permissions::from_mode(0o700))
        .expect("private planner directory");
    let (depositor, depositor_key, _) = account(21);
    let (claimant, claimant_key, _) = account(22);
    let descriptor = runtime(depositor);
    let xmr_terms = terms(depositor, claimant);
    let prepare = prepare_request(descriptor.clone(), &xmr_terms);
    let first_nonce = Arc::new(CountingNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let first_planner = NativeEscrowPlanner::new_durable(
        Participant::Taker,
        depositor_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        descriptor.clone(),
        Arc::clone(&first_nonce),
        &planner_directory,
    )
    .expect("planner");
    let prepared = first_planner
        .prepare_native_xmr_escrow_v3(&prepare)
        .await
        .expect("exact persisted pair");
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    drop(first_planner);

    let (_, restart_key, _) = account(21);
    let restart_nonce = Arc::new(CountingNonce {
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let restarted_planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Taker,
            restart_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::clone(&restart_nonce),
            &planner_directory,
        )
        .expect("restarted planner"),
    );
    assert_eq!(
        restarted_planner
            .prepare_native_xmr_escrow_v3(&prepare)
            .await
            .expect("byte-identical recovery"),
        prepared
    );
    assert_eq!(restart_nonce.calls.load(Ordering::SeqCst), 0);

    let indexer = finalized_indexer(&prepared, &xmr_terms, depositor);
    let (node_endpoint, node, sends) = start_node().await;
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        Arc::new(OfficialNodeRpc::connect(&node_endpoint).expect("official node")),
        indexer.clone(),
    ));
    let bridge = start_bridge_server(
        BridgeServerConfig::new(
            RunId::new(RUN_ID).expect("run id"),
            BridgeServerCapability::new(CAPABILITY).expect("server capability"),
            directory.path().join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .expect("sidecar starts");
    let client = BridgeClient::connect(BridgeClientConfig::new(
        bridge.endpoint(),
        SidecarCapability::new(CAPABILITY).expect("client capability"),
        RunId::new(RUN_ID).expect("run id"),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .expect("client");
    let request = ClassifyFinalizedNativeXmrEffectV3Request::new(
        MessageContext::new(
            RunId::new(RUN_ID).expect("run id"),
            RequestId::new("xmr-fund-classify").expect("request id"),
            Participant::Taker,
        ),
        descriptor.clone(),
        xmr_terms,
        XmrNativeEffectV3::Fund,
        FinalizedNativeXmrTransactionTargetV3::exact(prepared.funding.clone()),
        DiscoveryWindow::new(FUNDING_BLOCK, 2).expect("window"),
    );

    let classified = client
        .classify_finalized_native_xmr_effect_v3(request)
        .await
        .expect("authenticated classifier");

    let FinalizedNativeXmrScanOutcomeV3::Found {
        finalized_clock,
        scanned_window,
        facts,
    } = classified.outcome
    else {
        panic!("exact persisted Fund must be found")
    };
    assert_eq!(finalized_clock.height, FINALIZED_END);
    assert_eq!(
        scanned_window,
        DiscoveryWindow::new(FUNDING_BLOCK, 2).unwrap()
    );
    assert_eq!(
        facts.transaction.transaction_id,
        prepared.funding.transaction_id
    );
    assert_eq!(facts.transaction.exact_bytes, prepared.funding.exact_bytes);
    assert_eq!(facts.instruction.effect, XmrNativeEffectV3::Fund);
    assert_eq!(facts.instruction.swap_id, Hex32::from_bytes(SWAP_ID));
    assert_eq!(facts.containing_block.block_id, FUNDING_BLOCK);
    assert_eq!(facts.aggregate_signature, None);
    assert_eq!(
        facts.metadata,
        XmrNativeEscrowMetadataFactsV3::from_terms(xmr_terms, XmrNativeEscrowStateV3::Funded,)
    );
    assert_eq!(facts.custody.balance.as_u128(), 75);
    {
        let calls = indexer.calls.lock().expect("calls lock");
        assert!(calls.iter().filter(|call| *call == "id:10").count() >= 3);
        assert!(calls.iter().filter(|call| *call == "id:11").count() >= 3);
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("account:"))
                .count(),
            2
        );
        assert_eq!(sends.load(Ordering::SeqCst), 0);
    }
    bridge.stop().await.expect("sidecar stops");

    let initialization_indexer = finalized_initialization_indexer(&prepared, &xmr_terms, depositor);
    let (initialization_client, initialization_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-initialize-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        initialization_indexer.clone(),
    )
    .await;
    let initialized = initialization_client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            MessageContext::new(
                RunId::new(RUN_ID).expect("run id"),
                RequestId::new("xmr-initialize-classify").expect("request id"),
                Participant::Taker,
            ),
            descriptor.clone(),
            xmr_terms,
            XmrNativeEffectV3::Initialize,
            FinalizedNativeXmrTransactionTargetV3::exact(prepared.initialization.clone()),
            DiscoveryWindow::new(FUNDING_BLOCK, 2).expect("window"),
        ))
        .await
        .expect("authenticated Initialize classifier");
    let FinalizedNativeXmrScanOutcomeV3::Found {
        finalized_clock,
        scanned_window,
        facts,
    } = initialized.outcome
    else {
        panic!("exact persisted Initialize must be found")
    };
    assert_eq!(finalized_clock.height, FINALIZED_END);
    assert_eq!(
        scanned_window,
        DiscoveryWindow::new(FUNDING_BLOCK, 2).unwrap()
    );
    assert_eq!(
        facts.transaction.transaction_id,
        prepared.initialization.transaction_id
    );
    assert_eq!(
        facts.transaction.exact_bytes,
        prepared.initialization.exact_bytes
    );
    assert_eq!(facts.instruction.effect, XmrNativeEffectV3::Initialize);
    let input = xmr_terms.to_input();
    assert_eq!(
        facts.instruction.ordered_account_ids.as_slice(),
        [
            input.metadata_account_id,
            input.custody_account_id,
            input.depositor_account_id,
            input.claimant_account_id,
            input.claim_authority_account_id,
            input.refund_authority_account_id,
        ]
    );
    assert_eq!(
        facts.transaction.signer_account_ids.as_slice(),
        [input.depositor_account_id]
    );
    assert_eq!(facts.containing_block.block_id, FUNDING_BLOCK);
    assert_eq!(facts.aggregate_signature, None);
    assert_eq!(
        facts.metadata,
        XmrNativeEscrowMetadataFactsV3::from_terms(xmr_terms, XmrNativeEscrowStateV3::Empty)
    );
    assert_eq!(facts.custody.balance.as_u128(), 0);
    {
        let calls = initialization_indexer.calls.lock().expect("calls lock");
        assert!(calls.iter().filter(|call| *call == "id:10").count() >= 3);
        assert!(calls.iter().filter(|call| *call == "id:11").count() >= 3);
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("account:"))
                .count(),
            2
        );
    }
    initialization_bridge
        .stop()
        .await
        .expect("initialization sidecar stops");

    let missing_initialization_indexer = missing_fund_indexer(&xmr_terms, EscrowStatus::Empty);
    let (missing_initialization_client, missing_initialization_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-initialize-missing-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        missing_initialization_indexer,
    )
    .await;
    let missing_initialization = missing_initialization_client
        .classify_finalized_native_xmr_effect_v3(classification_request_for_effect(
            &descriptor,
            &xmr_terms,
            XmrNativeEffectV3::Initialize,
            &prepared.initialization,
            "xmr-initialize-missing",
        ))
        .await
        .expect("missing Initialize remains typed");
    assert!(matches!(
        missing_initialization.outcome,
        FinalizedNativeXmrScanOutcomeV3::Uncertain {
            finalized_clock,
            scanned_window,
        } if finalized_clock.height == FINALIZED_END
            && scanned_window == DiscoveryWindow::new(FUNDING_BLOCK, 2).unwrap()
    ));
    missing_initialization_bridge
        .stop()
        .await
        .expect("missing Initialize sidecar stops");

    let moving_initialization_indexer =
        moving_initialization_end_indexer(&prepared, &xmr_terms, depositor);
    let (moving_initialization_client, moving_initialization_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-initialize-moving-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        moving_initialization_indexer,
    )
    .await;
    let moving_initialization = moving_initialization_client
        .classify_finalized_native_xmr_effect_v3(classification_request_for_effect(
            &descriptor,
            &xmr_terms,
            XmrNativeEffectV3::Initialize,
            &prepared.initialization,
            "xmr-initialize-moving",
        ))
        .await
        .expect("moving Initialize tip is typed");
    assert_eq!(
        moving_initialization.outcome,
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::MovingTip,
        )
    );
    moving_initialization_bridge
        .stop()
        .await
        .expect("moving Initialize sidecar stops");

    for (name, stage, reason) in [
        (
            "finality",
            UnavailableStage::Finality,
            FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
        ),
        (
            "history",
            UnavailableStage::History,
            FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
        ),
    ] {
        let (unavailable_client, unavailable_bridge) = start_classifier_sidecar(
            directory.path(),
            &format!("bridge-initialize-{name}-idempotency.json"),
            descriptor.clone(),
            Arc::clone(&restarted_planner),
            &node_endpoint,
            Arc::new(UnavailableEvidenceIndexer { stage }),
        )
        .await;
        let unavailable = unavailable_client
            .classify_finalized_native_xmr_effect_v3(classification_request_for_effect(
                &descriptor,
                &xmr_terms,
                XmrNativeEffectV3::Initialize,
                &prepared.initialization,
                &format!("xmr-initialize-{name}"),
            ))
            .await
            .expect("unavailable Initialize evidence is typed");
        assert_eq!(
            unavailable.outcome,
            FinalizedNativeXmrScanOutcomeV3::unavailable(reason)
        );
        unavailable_bridge
            .stop()
            .await
            .expect("unavailable Initialize sidecar stops");
    }

    for (name, invalid_indexer) in [
        (
            "accounts",
            malformed_initialization_indexer(&prepared, &xmr_terms, depositor),
        ),
        (
            "proof",
            proof_bearing_initialization_indexer(&prepared, &xmr_terms, depositor),
        ),
        (
            "metadata-state",
            initialization_state_indexer(&prepared, &xmr_terms, depositor, EscrowStatus::Funded, 0),
        ),
        (
            "custody-balance",
            initialization_state_indexer(
                &prepared,
                &xmr_terms,
                depositor,
                EscrowStatus::Empty,
                xmr_terms.to_input().amount,
            ),
        ),
    ] {
        let (invalid_client, invalid_bridge) = start_classifier_sidecar(
            directory.path(),
            &format!("bridge-invalid-initialize-{name}-idempotency.json"),
            descriptor.clone(),
            Arc::clone(&restarted_planner),
            &node_endpoint,
            invalid_indexer,
        )
        .await;
        let error = invalid_client
            .classify_finalized_native_xmr_effect_v3(classification_request_for_effect(
                &descriptor,
                &xmr_terms,
                XmrNativeEffectV3::Initialize,
                &prepared.initialization,
                &format!("xmr-invalid-initialize-{name}"),
            ))
            .await
            .expect_err("malformed Initialize facts fail closed");
        assert_remote_code(error, ErrorCode::InvalidTransaction);
        invalid_bridge
            .stop()
            .await
            .expect("invalid Initialize sidecar stops");
    }

    let absent_indexer = missing_fund_indexer(&xmr_terms, EscrowStatus::Empty);
    let (absent_client, absent_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-absent-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        absent_indexer.clone(),
    )
    .await;
    let absent = absent_client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            MessageContext::new(
                RunId::new(RUN_ID).expect("run id"),
                RequestId::new("xmr-fund-absent").expect("request id"),
                Participant::Taker,
            ),
            descriptor.clone(),
            xmr_terms,
            XmrNativeEffectV3::Fund,
            FinalizedNativeXmrTransactionTargetV3::exact(prepared.funding.clone()),
            DiscoveryWindow::new(FUNDING_BLOCK, 2).expect("window"),
        ))
        .await
        .expect("stable predecessor classification");
    assert!(matches!(
        absent.outcome,
        FinalizedNativeXmrScanOutcomeV3::Uncertain {
            finalized_clock,
            scanned_window,
        } if finalized_clock.height == FINALIZED_END
            && scanned_window == DiscoveryWindow::new(FUNDING_BLOCK, 2).unwrap()
    ));
    {
        let absent_calls = absent_indexer.calls.lock().expect("calls lock");
        assert!(absent_calls.iter().filter(|call| *call == "id:11").count() >= 3);
        assert_eq!(
            absent_calls
                .iter()
                .filter(|call| call.starts_with("account:"))
                .count(),
            2
        );
    }
    absent_bridge.stop().await.expect("absent sidecar stops");

    let uncertain_indexer = missing_fund_indexer(&xmr_terms, EscrowStatus::Funded);
    let (uncertain_client, uncertain_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-uncertain-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        uncertain_indexer,
    )
    .await;
    let uncertain = uncertain_client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            MessageContext::new(
                RunId::new(RUN_ID).expect("run id"),
                RequestId::new("xmr-fund-uncertain").expect("request id"),
                Participant::Taker,
            ),
            descriptor.clone(),
            xmr_terms,
            XmrNativeEffectV3::Fund,
            FinalizedNativeXmrTransactionTargetV3::exact(prepared.funding.clone()),
            DiscoveryWindow::new(FUNDING_BLOCK, 2).expect("window"),
        ))
        .await
        .expect("stable non-predecessor classification");
    assert!(matches!(
        uncertain.outcome,
        FinalizedNativeXmrScanOutcomeV3::Uncertain {
            finalized_clock,
            scanned_window,
        } if finalized_clock.height == FINALIZED_END
            && scanned_window == DiscoveryWindow::new(FUNDING_BLOCK, 2).unwrap()
    ));
    uncertain_bridge
        .stop()
        .await
        .expect("uncertain sidecar stops");

    let absent_state_indexer = absent_state_indexer(&xmr_terms);
    let (absent_state_client, absent_state_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-absent-state-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        absent_state_indexer,
    )
    .await;
    let absent_state = absent_state_client
        .classify_finalized_native_xmr_effect_v3(classification_request(
            &descriptor,
            &xmr_terms,
            &prepared.funding,
            "xmr-fund-absent-state",
        ))
        .await
        .expect("fully absent accounts remain non-affirmative");
    assert!(matches!(
        absent_state.outcome,
        FinalizedNativeXmrScanOutcomeV3::Uncertain {
            finalized_clock,
            scanned_window,
        } if finalized_clock.height == FINALIZED_END
            && scanned_window == DiscoveryWindow::new(FUNDING_BLOCK, 2).unwrap()
    ));
    absent_state_bridge
        .stop()
        .await
        .expect("absent-state sidecar stops");

    for (name, stage, reason) in [
        (
            "finality",
            UnavailableStage::Finality,
            FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
        ),
        (
            "history",
            UnavailableStage::History,
            FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
        ),
    ] {
        let (typed_client, typed_bridge) = start_classifier_sidecar(
            directory.path(),
            &format!("bridge-{name}-idempotency.json"),
            descriptor.clone(),
            Arc::clone(&restarted_planner),
            &node_endpoint,
            Arc::new(UnavailableEvidenceIndexer { stage }),
        )
        .await;
        let result = typed_client
            .classify_finalized_native_xmr_effect_v3(classification_request(
                &descriptor,
                &xmr_terms,
                &prepared.funding,
                &format!("xmr-fund-{name}"),
            ))
            .await
            .expect("typed unavailable result");
        assert_eq!(
            result.outcome,
            FinalizedNativeXmrScanOutcomeV3::unavailable(reason)
        );
        typed_bridge.stop().await.expect("typed sidecar stops");
    }

    let moving_indexer = moving_end_indexer(&xmr_terms);
    let (moving_client, moving_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-moving-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        moving_indexer,
    )
    .await;
    let moving = moving_client
        .classify_finalized_native_xmr_effect_v3(classification_request(
            &descriptor,
            &xmr_terms,
            &prepared.funding,
            "xmr-fund-moving",
        ))
        .await
        .expect("moving tip is typed");
    assert_eq!(
        moving.outcome,
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::MovingTip,
        )
    );
    moving_bridge.stop().await.expect("moving sidecar stops");

    let regression_indexer = regressing_finality_indexer(&prepared, &xmr_terms, depositor);
    let (regression_client, regression_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-finality-regression-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        regression_indexer,
    )
    .await;
    let regression = regression_client
        .classify_finalized_native_xmr_effect_v3(classification_request(
            &descriptor,
            &xmr_terms,
            &prepared.funding,
            "xmr-fund-finality-regression",
        ))
        .await
        .expect("finalized coverage regression is typed");
    assert_eq!(
        regression.outcome,
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
        )
    );
    regression_bridge
        .stop()
        .await
        .expect("finality-regression sidecar stops");

    let conflict_indexer = conflicting_fund_indexer(&prepared, &xmr_terms, depositor);
    let (conflict_client, conflict_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-conflict-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        conflict_indexer,
    )
    .await;
    let conflict = conflict_client
        .classify_finalized_native_xmr_effect_v3(classification_request(
            &descriptor,
            &xmr_terms,
            &prepared.funding,
            "xmr-fund-conflict",
        ))
        .await
        .expect("duplicate exact matches are typed");
    assert_eq!(
        conflict.outcome,
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::ConflictingMatches,
        )
    );
    conflict_bridge
        .stop()
        .await
        .expect("conflict sidecar stops");

    for (name, invalid_indexer) in [
        (
            "accounts",
            malformed_same_target_indexer(&prepared, &xmr_terms, depositor),
        ),
        (
            "proof",
            proof_bearing_fund_indexer(&prepared, &xmr_terms, depositor),
        ),
        ("one-sided", one_sided_missing_indexer(&xmr_terms)),
    ] {
        let (invalid_client, invalid_bridge) = start_classifier_sidecar(
            directory.path(),
            &format!("bridge-invalid-{name}-idempotency.json"),
            descriptor.clone(),
            Arc::clone(&restarted_planner),
            &node_endpoint,
            invalid_indexer,
        )
        .await;
        let error = invalid_client
            .classify_finalized_native_xmr_effect_v3(classification_request(
                &descriptor,
                &xmr_terms,
                &prepared.funding,
                &format!("xmr-fund-invalid-{name}"),
            ))
            .await
            .expect_err("malformed canonical facts fail closed");
        assert_remote_code(error, ErrorCode::InvalidTransaction);
        invalid_bridge
            .stop()
            .await
            .expect("invalid-facts sidecar stops");
    }

    let exact_funding = decode_prepared_for_signer(&prepared.funding, depositor)
        .expect("persisted funding decodes");
    let mut alternate_message = exact_funding.message().clone();
    alternate_message.nonces[0] = 43_u128.into();
    let (_, alternate_key, alternate_public) = account(21);
    let alternate_signature = Signature::new(&alternate_key, &alternate_message.hash());
    let alternate_funding = prepared_from_transaction(&PublicTransaction::new(
        alternate_message,
        WitnessSet::from_raw_parts(vec![(alternate_signature, alternate_public)]),
    ))
    .expect("alternate valid signed Fund");
    let ownership_indexer = finalized_indexer(&prepared, &xmr_terms, depositor);
    let (ownership_client, ownership_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-ownership-idempotency.json",
        descriptor.clone(),
        Arc::clone(&restarted_planner),
        &node_endpoint,
        ownership_indexer.clone(),
    )
    .await;
    let ownership_error = ownership_client
        .classify_finalized_native_xmr_effect_v3(classification_request(
            &descriptor,
            &xmr_terms,
            &alternate_funding,
            "xmr-fund-unowned-target",
        ))
        .await
        .expect_err("another valid signed Fund is not the durable target");
    assert_remote_code(ownership_error, ErrorCode::InvalidTransaction);
    assert!(
        ownership_indexer
            .calls
            .lock()
            .expect("calls lock")
            .is_empty(),
        "durable ownership must fail before evidence reads"
    );
    ownership_bridge
        .stop()
        .await
        .expect("ownership sidecar stops");

    let maker_directory = directory.path().join("maker-planner");
    fs::create_dir(&maker_directory).expect("maker planner directory");
    fs::set_permissions(&maker_directory, fs::Permissions::from_mode(0o700))
        .expect("private maker planner directory");
    let maker_descriptor = runtime_for(Participant::Maker, claimant);
    let maker_nonce = Arc::new(CountingNonce {
        value: 77,
        calls: AtomicUsize::new(0),
    });
    let maker_planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            claimant_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            maker_descriptor.clone(),
            Arc::clone(&maker_nonce),
            &maker_directory,
        )
        .expect("maker planner"),
    );
    let maker_indexer = finalized_indexer(&prepared, &xmr_terms, depositor);
    let (maker_client, maker_bridge) = start_classifier_sidecar(
        directory.path(),
        "bridge-maker-idempotency.json",
        maker_descriptor.clone(),
        maker_planner,
        &node_endpoint,
        maker_indexer.clone(),
    )
    .await;
    let maker_error = maker_client
        .classify_finalized_native_xmr_effect_v3(classification_request(
            &maker_descriptor,
            &xmr_terms,
            &prepared.funding,
            "xmr-fund-maker-without-reservation",
        ))
        .await
        .expect_err("Maker cannot claim Taker durable target ownership");
    assert_remote_code(maker_error, ErrorCode::InvalidTransaction);
    assert!(maker_indexer.calls.lock().expect("calls lock").is_empty());
    assert_eq!(maker_nonce.calls.load(Ordering::SeqCst), 0);
    maker_bridge.stop().await.expect("maker sidecar stops");

    assert_eq!(sends.load(Ordering::SeqCst), 0);

    node.stop().expect("node stops");
    node.stopped().await;
}
