# Logos-native acceptance — maker (20261111T095319Z)

Verdicts: PASS / FAIL against the target contract; INFO records a fact; NOT RUN says why not.
Every command ran inside a throwaway container (debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4) on an internal network.

## Tools

### S01 · PASS · logosctl version

```
[run] $ logosctl --version
logosctl version 0.3.2
commit: 96e704df1a8140f069bbf6d5a1cdeab8886c6e41
dependency commits:
  logos-liblogos 00f19ef8e2ffd54e2cf97616a960cd19232ad4ba
  logos-cpp-sdk 3bf5365c5412f89bbbf30e3f46237cd68cff0599
  logos-protocol 9d59f5de526624b120d42f4c422a3d83c0b9d64f
  logos-plugin-qt 2d36a08819b8df36094036d02d130aa4bd3a6132
  logos-capability-module 1a1b8b5a1167931afbf41ebaafbe76034e6357a0
  logos-modules-state-module c70da49aa56e48f867d50b9ff58679749f09dbcf
  logos-package-manager-module 4c5fa074489f1977f57d6ae0b0538b379c4428d4
  logos-package-downloader-module 6e8883b6918e086183f797d522c32032a55d2354
  logos-storage-module 7548f41ae189917d58c37f0fb6d3a2700b99f59b
[exit 0]
```

### S02 · PASS · lm version (v0.2-era, from the module builder's nix store)

```
[tools] $ /x/lm --version
lm (Logos Module) version 0.1.0
[exit 0]
```

### S03 · PASS · lgx version (v0.2-era, from the module builder's nix store)

```
[tools] $ /x/lgx --version
lgx version 0.1.0
[exit 0]
```

### S04 · PASS · lgpm version (v0.2-era, from the module builder's nix store)

```
[tools] $ /x/lgpm --version
lgpm version 1.0.0
[exit 0]
```

### S05 · PASS · logoscore version (the v0.2-era runtime in our images)

```
[tools] $ logoscore --version
logoscore version 0.2.3
commit: a09b2d1ff0a9b6af7dbd024a512d68a26f1b4445
dependency commits:
  logos-liblogos 7234c268456836895690611867c844c5ab2ce542
  logos-cpp-sdk 937f17ed018fb942ad6902456e871ce5e8b9becc
  logos-protocol 42460e5b2a30c0f97ea95c679e5a8e8a08a4af1f
  logos-plugin-qt 048152f2a17b41a1cab5d1c46f519a29697b3eb5
  logos-capability-module 3ed67bcdd695d04c360432a889d20dba9c46493f
  logos-package-manager-module b19c0356fc5e0bd78a4b6afff51b013b7a314188
  logos-package-downloader-module e226c6b5513339903784791b1c88a240516a1bb6
[exit 0]
```

Host architecture: x86_64. Base image: debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4.
logosctl 0.3.2 is published for linux x86_64/aarch64, macOS aarch64 and Windows x86_64; lm/lgx/lgpm have no released binaries.

## Inputs

```
name                   file sha256                                                       version       manifestVersion  type    variants                              dependencies                                                       root hash
lez_atomic_swap_maker  760c489e8e2b71d0f39881c315671515bf20798754348f474c658e650f108735  0.2.5         0.3.0            ui_qml  linux-amd64-dev                       chat_module,delivery_module,lez_core,lez_swap_node,lez_maker_node  cdc00bd5366c84e1c323b7f31b1b80e5ba45470d5426e4c9cbd04e8c603c2422
lez_core               12f508bcae8de5f042360d934d5c253668f1b5c693292aa4cfc6c1bc0300ba36  0.4.1-sign.4  0.3.0            core    linux-amd64                                                                                              8872f8e34689b5ae0953423b57a2b317ed748fd03a4c46e232049d182c792edf
lez_swap_node          e023639a7cacff58a3dfb24739f67208eed0cb16fcd6d93e2425cb38808fdf28  0.3.0         0.2.0            core    linux-amd64                           lez_core                                                           a9e04c9f7dca3c7a18163ed7380a70e5b40e3ac83b280ed5248de9edd40da104
lez_maker_node         001a554d42ee74b612f974591a86d98b0601fb5639c95a11982ca302ea3d6754  0.1.0         0.2.0            core    linux-amd64                           lez_swap_node                                                      939e318ea2ab413da35520d2f179916ea3e4bab920e722658d96fb3fc0e0d99e
chat_module            1a0b03365f4062f36b0b8d9fd4c41e082196dbe3f00440cc9c36ee382873fb8b  0.2.2         0.3.0            core    linux-amd64-dev                       delivery_module                                                    e4c0346cf47d420d9a05a8f9a1f0867d232c2546c9c7102df86c66c739b5a499
delivery_module        9b9e0238fde66d103c3dc2ffcd7d101cef3e984d98be3120e77d26d77dec4b2e  0.2.0         0.3.0            core    linux-amd64-dev                                                                                          36eaa5da4e5f47bf111fd39e1ea42bba67f69f1018177532fa2e97b87bf8562f
lez_indexer_module     c40df9b7ec056d75c67bca2ae51768c68cf712358af39bbecc7ed758fa428239  1.1.1-rpc.4   0.2.0            core    linux-amd64                                                                                              6288c56b37037a6875723e60a449a4f6d31036340ecaa224ecc9b50f7d0e187d
blockchain_module      31819f06aa0639874fa0257f58c24e1add6fce2b489c843b64905e28f59f0c84  0.2.4         0.3.0            core    darwin-arm64,linux-amd64,linux-arm64                                                                     2e57268c4ec1fdcf07e4b6bf1b33b5ac99705c071f879e6ca1c41b4e543cc674
```

## Package truth (before installing)

### S06 · PASS · start a clean session daemon

```
[run] $ logosctl --config-dir /tmp/acceptance daemon start --detach
Daemon started (pid 23)
Logs: /tmp/acceptance/logs/daemon.log
[exit 0]
```

### S07 · PASS · package show lez_atomic_swap_maker

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/lez_atomic_swap_maker.lgx
{"category":"finance","description":"Publish wallet-owned BTC/LEZ offers and perform Maker settlement actions.","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"lez_atomic_swap_maker","rootHash":"cdc00bd5366c84e1c323b7f31b1b80e5ba45470d5426e4c9cbd04e8c603c2422","signatureStatus":"unsigned","type":"ui_qml","variants":["linux-amd64-dev"],"version":"0.2.5"}
[exit 0]
```

### S08 · FAIL · lez_atomic_swap_maker is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: cdc00bd5366c84e1c323b7f31b1b80e5ba45470d5426e4c9cbd04e8c603c2422
```

### S09 · PASS · package show lez_core

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/lez_core.lgx
{"category":"blockchain","description":"Logos Execution Zone Core Module for Logos Core","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"lez_core","rootHash":"8872f8e34689b5ae0953423b57a2b317ed748fd03a4c46e232049d182c792edf","signatureStatus":"unsigned","type":"core","variants":["linux-amd64"],"version":"0.4.1-sign.4"}
[exit 0]
```

### S10 · FAIL · lez_core is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: 8872f8e34689b5ae0953423b57a2b317ed748fd03a4c46e232049d182c792edf
```

### S11 · PASS · package show lez_swap_node

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/lez_swap_node.lgx
{"category":"blockchain","description":"Launches and supervises a swap role's Node (the swap engine) inside Basecamp and relays its LEZ signing requests to Basecamp's wallet","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"lez_swap_node","rootHash":"a9e04c9f7dca3c7a18163ed7380a70e5b40e3ac83b280ed5248de9edd40da104","signatureStatus":"unsigned","type":"core","variants":["linux-amd64"],"version":"0.3.0"}
[exit 0]
```

### S12 · FAIL · lez_swap_node is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: a9e04c9f7dca3c7a18163ed7380a70e5b40e3ac83b280ed5248de9edd40da104
```

### S13 · PASS · package show lez_maker_node

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/lez_maker_node.lgx
{"category":"blockchain","description":"The Maker's Node (the swap engine) and everything it runs: the Maker Node, its CLI, its Bitcoin and Zcash actors and the LEZ v0.2 sidecar, with the launcher lez_swap_node starts it from. Linux only (linux-amd64, linux-arm64)","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"lez_maker_node","rootHash":"939e318ea2ab413da35520d2f179916ea3e4bab920e722658d96fb3fc0e0d99e","signatureStatus":"unsigned","type":"core","variants":["linux-amd64"],"version":"0.1.0"}
[exit 0]
```

### S14 · FAIL · lez_maker_node is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: 939e318ea2ab413da35520d2f179916ea3e4bab920e722658d96fb3fc0e0d99e
```

### S15 · PASS · package show chat_module

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/chat_module.lgx
{"category":"chat","description":"Chat module for Logos","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"chat_module","rootHash":"e4c0346cf47d420d9a05a8f9a1f0867d232c2546c9c7102df86c66c739b5a499","signatureStatus":"unsigned","type":"core","variants":["linux-amd64-dev"],"version":"0.2.2"}
[exit 0]
```

### S16 · FAIL · chat_module is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: e4c0346cf47d420d9a05a8f9a1f0867d232c2546c9c7102df86c66c739b5a499
```

### S17 · PASS · package show delivery_module

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/delivery_module.lgx
{"category":"protocol","description":"Logos Delivery Module - High-level message-delivery API","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"delivery_module","rootHash":"36eaa5da4e5f47bf111fd39e1ea42bba67f69f1018177532fa2e97b87bf8562f","signatureStatus":"unsigned","type":"core","variants":["linux-amd64-dev"],"version":"0.2.0"}
[exit 0]
```

### S18 · FAIL · delivery_module is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: 36eaa5da4e5f47bf111fd39e1ea42bba67f69f1018177532fa2e97b87bf8562f
```

### S19 · PASS · package show lez_indexer_module

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/lez_indexer_module.lgx
{"category":"blockchain","description":"Logos Execution Zone Indexer Module for Logos Core","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"lez_indexer_module","rootHash":"6288c56b37037a6875723e60a449a4f6d31036340ecaa224ecc9b50f7d0e187d","signatureStatus":"unsigned","type":"core","variants":["linux-amd64"],"version":"1.1.1-rpc.4"}
[exit 0]
```

### S20 · FAIL · lez_indexer_module is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: 6288c56b37037a6875723e60a449a4f6d31036340ecaa224ecc9b50f7d0e187d
```

### S21 · PASS · package show blockchain_module

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /in/blockchain_module.lgx
{"category":"blockchain","description":"Logos blockchain node for logos-core","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"blockchain_module","rootHash":"2e57268c4ec1fdcf07e4b6bf1b33b5ac99705c071f879e6ca1c41b4e543cc674","signatureStatus":"unsigned","type":"core","variants":["darwin-arm64","linux-amd64","linux-arm64"],"version":"0.2.4"}
[exit 0]
```

### S22 · FAIL · blockchain_module is signed by a signer that can be pinned

```
signatureStatus: unsigned; rootHash: 2e57268c4ec1fdcf07e4b6bf1b33b5ac99705c071f879e6ca1c41b4e543cc674
```

### S23 · PASS · lgx verify lez_atomic_swap_maker (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/lez_atomic_swap_maker.lgx
Package structure is valid: /in/lez_atomic_swap_maker.lgx
Package is unsigned
[exit 0]
```

### S24 · PASS · lgx verify lez_core (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/lez_core.lgx
Package structure is valid: /in/lez_core.lgx
Package is unsigned
[exit 0]
```

### S25 · PASS · lgx verify lez_swap_node (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/lez_swap_node.lgx
Package structure is valid: /in/lez_swap_node.lgx
Package is unsigned
[exit 0]
```

### S26 · PASS · lgx verify lez_maker_node (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/lez_maker_node.lgx
Package structure is valid: /in/lez_maker_node.lgx
Package is unsigned
[exit 0]
```

### S27 · PASS · lgx verify chat_module (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/chat_module.lgx
Package structure is valid: /in/chat_module.lgx
Package is unsigned
[exit 0]
```

### S28 · PASS · lgx verify delivery_module (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/delivery_module.lgx
Package structure is valid: /in/delivery_module.lgx
Package is unsigned
[exit 0]
```

### S29 · PASS · lgx verify lez_indexer_module (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/lez_indexer_module.lgx
Package structure is valid: /in/lez_indexer_module.lgx
Package is unsigned
[exit 0]
```

### S30 · PASS · lgx verify blockchain_module (structure, Merkle root)

```
[tools] $ /x/lgx verify /in/blockchain_module.lgx
Package structure is valid: /in/blockchain_module.lgx
Package is unsigned
[exit 0]
```

### S31 · PASS · lm metadata lez_atomic_swap_maker

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/lez_atomic_swap_maker/variants/linux-amd64-dev/lez_atomic_swap_maker_plugin.so
Plugin Metadata:
================
Name:         lez_atomic_swap_maker
Display name: LEZ Maker
Version:      0.2.5
Description:  Publish wallet-owned BTC/LEZ offers and perform Maker settlement actions.
Author:       
Type:         ui_qml
Protocol:     0.2.0
Dependencies: chat_module, delivery_module, lez_core, lez_swap_node, lez_maker_node
[exit 0]
```

### S32 · PASS · lm methods lez_atomic_swap_maker

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/lez_atomic_swap_maker/variants/linux-amd64-dev/lez_atomic_swap_maker_plugin.so
Plugin Methods:
===============

void initLogos(LogosAPI* api)
  Signature: initLogos(LogosAPI*)
  Invokable: yes

[exit 0]
```

### S33 · PASS · lm events lez_atomic_swap_maker

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/lez_atomic_swap_maker/variants/linux-amd64-dev/lez_atomic_swap_maker_plugin.so
Plugin Events:
==============

(no events found)
[exit 0]
```

### S34 · PASS · lm metadata lez_core

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/lez_core/variants/linux-amd64/lez_core_plugin.so
Plugin Metadata:
================
Name:         lez_core
Display name: LEZ Core Module
Version:      0.4.1-sign.4
Description:  Logos Execution Zone Core Module for Logos Core
Author:       Logos Blockchain Team
Type:         core
Protocol:     0.2.0
Dependencies: (none)
[exit 0]
```

### S35 · PASS · lm methods lez_core

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/lez_core/variants/linux-amd64/lez_core_plugin.so
Plugin Methods:
===============

QString name()
  Signature: name()
  Invokable: yes

QString version()
  Signature: version()
  Invokable: yes

QString create_new(QString config_path, QString storage_path, QString statistics_path, QString password)
  Signature: create_new(QString,QString,QString,QString)
  Invokable: yes

int open(QString config_path, QString storage_path, QString statistics_path)
  Signature: open(QString,QString,QString)
  Invokable: yes

int save()
  Signature: save()
  Invokable: yes

int restore_storage(QString mnemonic, QString password, QVariant depth)
  Signature: restore_storage(QString,QString,QVariant)
  Invokable: yes

QString create_account_public()
  Signature: create_account_public()
  Invokable: yes

QString create_account_private()
  Signature: create_account_private()
  Invokable: yes

int import_public_account(QString private_key_hex)
  Signature: import_public_account(QString)
  Invokable: yes

QString sign_public_message_hash(QString account_id_hex, QString message_hash_hex)
  Signature: sign_public_message_hash(QString,QString)
  Invokable: yes

QVariantList list_accounts()
  Signature: list_accounts()
  Invokable: yes

QString get_balance(QString account_id_hex, bool is_public)
  Signature: get_balance(QString,bool)
  Invokable: yes

QString get_account_public(QString account_id_hex)
  Signature: get_account_public(QString)
  Invokable: yes

QString get_account_private(QString account_id_hex)
  Signature: get_account_private(QString)
  Invokable: yes

QString get_public_account_key(QString account_id_hex)
  Signature: get_public_account_key(QString)
  Invokable: yes

QString get_private_account_keys(QString account_id_hex)
  Signature: get_private_account_keys(QString)
  Invokable: yes

QString account_id_to_base58(QString account_id_hex)
  Signature: account_id_to_base58(QString)
  Invokable: yes

QString account_id_from_base58(QString base58_str)
  Signature: account_id_from_base58(QString)
  Invokable: yes

int sync_to_block(int block_id)
  Signature: sync_to_block(int)
  Invokable: yes

int get_last_synced_block()
  Signature: get_last_synced_block()
  Invokable: yes

int get_current_block_height()
  Signature: get_current_block_height()
  Invokable: yes

QString claim_pinata(QString pinata_account_id_hex, QString winner_account_id_hex, QString solution_le16_hex)
  Signature: claim_pinata(QString,QString,QString)
  Invokable: yes

QString claim_pinata_private_owned_already_initialized(QString pinata_account_id_hex, QString winner_account_id_hex, QString solution_le16_hex, int winner_proof_index, QString winner_proof_siblings_json)
  Signature: claim_pinata_private_owned_already_initialized(QString,QString,QString,int,QString)
  Invokable: yes

QString claim_pinata_private_owned_not_initialized(QString pinata_account_id_hex, QString winner_account_id_hex, QString solution_le16_hex)
  Signature: claim_pinata_private_owned_not_initialized(QString,QString,QString)
  Invokable: yes

QString transfer_public(QString from_hex, QString to_hex, QString amount_le16_hex)
  Signature: transfer_public(QString,QString,QString)
  Invokable: yes

QString transfer_shielded(QString from_hex, QString to_keys_json, QString amount_le16_hex)
  Signature: transfer_shielded(QString,QString,QString)
  Invokable: yes

QString transfer_deshielded(QString from_hex, QString to_hex, QString amount_le16_hex)
  Signature: transfer_deshielded(QString,QString,QString)
  Invokable: yes

QString transfer_private(QString from_hex, QString to_keys_json, QString amount_le16_hex)
  Signature: transfer_private(QString,QString,QString)
  Invokable: yes

QString transfer_shielded_owned(QString from_hex, QString to_hex, QString amount_le16_hex)
  Signature: transfer_shielded_owned(QString,QString,QString)
  Invokable: yes

QString transfer_private_owned(QString from_hex, QString to_hex, QString amount_le16_hex)
  Signature: transfer_private_owned(QString,QString,QString)
  Invokable: yes

QString register_public_account(QString account_id_hex)
  Signature: register_public_account(QString)
  Invokable: yes

QString register_private_account(QString account_id_hex)
  Signature: register_private_account(QString)
  Invokable: yes

QByteArray authenticated_transfer_elf()
  Signature: authenticated_transfer_elf()
  Invokable: yes

QByteArray token_elf()
  Signature: token_elf()
  Invokable: yes

QByteArray amm_elf()
  Signature: amm_elf()
  Invokable: yes

QByteArray ata_elf()
  Signature: ata_elf()
  Invokable: yes

QString send_generic_public_transaction(QStringList account_ids, QVariantList signing_requirements, QByteArray instruction, QString program_id_hex)
  Signature: send_generic_public_transaction(QStringList,QVariantList,QByteArray,QString)
  Invokable: yes

QString send_generic_private_transaction(QStringList account_ids, QVariant instruction, QByteArray program_elf, QVariant program_dependencies)
  Signature: send_generic_private_transaction(QStringList,QVariant,QByteArray,QVariant)
  Invokable: yes

QString send_program_deployment_transaction(QByteArray program_elf)
  Signature: send_program_deployment_transaction(QByteArray)
  Invokable: yes

QString bridge_withdraw(QString from_hex, QString bedrock_account_pk_hex, int amount)
  Signature: bridge_withdraw(QString,QString,int)
  Invokable: yes

QString get_vault_balance(QString owner_account_id_hex)
  Signature: get_vault_balance(QString)
  Invokable: yes

QString vault_claim(QString owner_account_id_hex, QString amount_le16_hex)
  Signature: vault_claim(QString,QString)
  Invokable: yes

QString vault_claim_private(QString owner_account_id_hex, QString amount_le16_hex)
  Signature: vault_claim_private(QString,QString)
  Invokable: yes

QString get_sequencer_addr()
  Signature: get_sequencer_addr()
  Invokable: yes

bool check_label_available(QString label)
  Signature: check_label_available(QString)
  Invokable: yes

int add_label(QString label, QString account_id_hex, bool is_private)
  Signature: add_label(QString,QString,bool)
  Invokable: yes

QString resolve_label(QString label)
  Signature: resolve_label(QString)
  Invokable: yes

QStringList get_all_labels_for_account(QString account_id_hex, bool is_private)
  Signature: get_all_labels_for_account(QString,bool)
  Invokable: yes

[exit 0]
```

### S36 · PASS · lm events lez_core

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/lez_core/variants/linux-amd64/lez_core_plugin.so
Plugin Events:
==============

(no events found)
[exit 0]
```

### S37 · PASS · lm metadata lez_swap_node

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/lez_swap_node/variants/linux-amd64/lez_swap_node_plugin.so
Plugin Metadata:
================
Name:         lez_swap_node
Display name: LEZ Swap Node
Version:      0.3.0
Description:  Launches and supervises a swap role's Node (the swap engine) inside Basecamp and relays its LEZ signing requests to Basecamp's wallet
Author:       
Type:         core
Protocol:     0.1.0
Dependencies: lez_core
[exit 0]
```

### S38 · PASS · lm methods lez_swap_node

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/lez_swap_node/variants/linux-amd64/lez_swap_node_plugin.so
Plugin Methods:
===============

int start_node(QString role, QString launcher, QString log_file, QString hold_file)
  Signature: start_node(QString,QString,QString,QString)
  Invokable: yes
  Description:
    Start supervising `role`'s Node: `launcher` (absolute, executable) run
    with `role` as its one argument, output appended to `log_file`, held
    down while `hold_file` exists. Idempotent: a role already supervised is
    left as it is. Returns 0 when supervised, else non-zero (1 invalid
    arguments). int64_t: the universal codegen marshals it as a scalar.

int stop_node(QString role)
  Signature: stop_node(QString)
  Invokable: yes
  Description:
    Stop `role`'s Node (SIGTERM to its process group, SIGKILL after 10 s),
    stop supervising it and forget it (a reload does not start it again).
    Returns 0, also for a role not supervised.

QString node_status(QString role)
  Signature: node_status(QString)
  Invokable: yes
  Description:
    `role`'s supervision as compact JSON: {role, state (running, held,
    restarting, stopped), pid, starts, last_exit}; empty when the role was
    never started.

int serve_signer(QString socket_path, QString account_id_hex)
  Signature: serve_signer(QString,QString)
  Invokable: yes
  Description:
    Serve `account_id_hex`'s LEZ signatures to the role's Node on the
    owner-only Unix socket `socket_path`: one JSON line per connection,
    {"account_id", "message_hash"} in, the wallet's {"signature",
    "public_key"} (or {"error"}) out. Every signature is made by Basecamp's
    LEZ wallet (lez_core, sign_public_message_hash); the key never reaches
    this module or the Node. Only that one account is served, and a socket
    already served is left as it is. Returns 0 when serving, else non-zero.

[exit 0]
```

### S39 · PASS · lm events lez_swap_node

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/lez_swap_node/variants/linux-amd64/lez_swap_node_plugin.so
Plugin Events:
==============

(no events found)
[exit 0]
```

### S40 · PASS · lm metadata lez_maker_node

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/lez_maker_node/variants/linux-amd64/lez_maker_node_plugin.so
Plugin Metadata:
================
Name:         lez_maker_node
Display name: LEZ Maker Node
Version:      0.1.0
Description:  The Maker's Node (the swap engine) and everything it runs: the Maker Node, its CLI, its Bitcoin and Zcash actors and the LEZ v0.2 sidecar, with the launcher lez_swap_node starts it from. Linux only (linux-amd64, linux-arm64)
Author:       
Type:         core
Protocol:     0.1.0
Dependencies: lez_swap_node
[exit 0]
```

### S41 · PASS · lm methods lez_maker_node

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/lez_maker_node/variants/linux-amd64/lez_maker_node_plugin.so
Plugin Methods:
===============

QString launcher()
  Signature: launcher()
  Invokable: yes
  Description:
    The installed launcher's absolute path; empty when this package has
    no executable launcher (a damaged install).

QString payload()
  Signature: payload()
  Invokable: yes
  Description: The installed payload's `bin/payload.json`; empty when absent.

[exit 0]
```

### S42 · PASS · lm events lez_maker_node

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/lez_maker_node/variants/linux-amd64/lez_maker_node_plugin.so
Plugin Events:
==============

(no events found)
[exit 0]
```

### S43 · PASS · lm metadata chat_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/chat_module/variants/linux-amd64-dev/chat_module_plugin.so
Plugin Metadata:
================
Name:         chat_module
Display name: Chat Module
Version:      0.2.2
Description:  Chat module for Logos
Author:       Logos Core Team
Type:         core
Protocol:     0.2.0
Dependencies: delivery_module
[exit 0]
```

### S44 · PASS · lm methods chat_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/chat_module/variants/linux-amd64-dev/chat_module_plugin.so
Plugin Methods:
===============

LogosResult init(QVariant config)
  Signature: init(QVariant)
  Invokable: yes

LogosResult shutdown()
  Signature: shutdown()
  Invokable: yes

QString get_log_path()
  Signature: get_log_path()
  Invokable: yes

bool health()
  Signature: health()
  Invokable: yes

QString get_installation_name()
  Signature: get_installation_name()
  Invokable: yes

LogosResult set_installation_name(QString name)
  Signature: set_installation_name(QString)
  Invokable: yes

QString get_address()
  Signature: get_address()
  Invokable: yes

LogosResult create_conversation(QString peer_address)
  Signature: create_conversation(QString)
  Invokable: yes

LogosResult create_group_conversation(QString name, QString desc)
  Signature: create_group_conversation(QString,QString)
  Invokable: yes

LogosResult add_group_member(QString convo_id, QString peer_address)
  Signature: add_group_member(QString,QString)
  Invokable: yes

QVariantList list_conversations()
  Signature: list_conversations()
  Invokable: yes

QVariantList get_messages(QString convo_id)
  Signature: get_messages(QString)
  Invokable: yes

QVariantList list_group_members(QString convo_id)
  Signature: list_group_members(QString)
  Invokable: yes

LogosResult send_message(QString convo_id, QString content)
  Signature: send_message(QString,QString)
  Invokable: yes

LogosResult set_conversation_nickname(QString convo_id, QString nickname)
  Signature: set_conversation_nickname(QString,QString)
  Invokable: yes

LogosResult delete_conversation(QString convo_id)
  Signature: delete_conversation(QString)
  Invokable: yes

QVariant status()
  Signature: status()
  Invokable: yes

[exit 0]
```

### S45 · PASS · lm events chat_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/chat_module/variants/linux-amd64-dev/chat_module_plugin.so
Plugin Events:
==============

void message_received(QString convo_id, QString content, int timestamp_ms, QString sender)
  Signature: message_received(QString,QString,int,QString)

void message_sent(QString convo_id, QString content, int timestamp_ms)
  Signature: message_sent(QString,QString,int)

void conversation_created(QString convo_id, bool is_outgoing, QString peer_label, QString kind, QString name, QString desc)
  Signature: conversation_created(QString,bool,QString,QString,QString,QString)

void conversation_updated(QString convo_id)
  Signature: conversation_updated(QString)

void members_changed(QString convo_id)
  Signature: members_changed(QString)

void conversation_deleted(QString convo_id)
  Signature: conversation_deleted(QString)

void delivery_state_changed(QString delivery_state, QString detail)
  Signature: delivery_state_changed(QString,QString)

[exit 0]
```

### S46 · PASS · lm metadata delivery_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/delivery_module/variants/linux-amd64-dev/delivery_module_plugin.so
Plugin Metadata:
================
Name:         delivery_module
Display name: (unset — falls back to name)
Version:      0.2.0
Description:  Logos Delivery Module - High-level message-delivery API
Author:       Logos Core Team
Type:         core
Protocol:     0.2.0
Dependencies: (none)
[exit 0]
```

### S47 · PASS · lm methods delivery_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/delivery_module/variants/linux-amd64-dev/delivery_module_plugin.so
Plugin Methods:
===============

LogosResult createNode(QString cfg)
  Signature: createNode(QString)
  Invokable: yes
  Description:
    @brief Creates a liblogosdelivery node from a JSON configuration.
    
    The JSON passes through to logos-delivery verbatim; `parseLogosDeliveryConf`
    (https://github.com/logos-messaging/logos-delivery) owns the grammar.
    `entryLayer` selects how much of the stack is mounted:
    - `"kernel"` — transport node only
    - `"messaging"` — kernel + messaging client
    - `"channels"` — kernel + messaging + reliable channels (default)
    
    Three typical shapes:
    
    **App developer** — full stack (default `entryLayer`). `preset` picks the
    network (`"logos.test"`, `"logos.dev"`, `"twn"`), `mode` picks the protocol
    flags (`"Core"` = relay node, `"Edge"` = light node). Optional
    `messagingOverrides` / `channelsOverrides` objects override per-layer
    defaults:
    @code{.json}
    { "mode": "Core", "preset": "logos.test" }
    @endcode
    
    **Node operator** — kernel-only service node on a public network. `mode`
    is not applied on this layer, so protocol flags are set explicitly in
    `kernelConf`:
    @code{.json}
    {
    "entryLayer": "kernel",
    "kernelConf": { "preset": "logos.test", "relay": true }
    }
    @endcode
    
    **Network hoster** — kernel-only node on a self-hosted network;
    `kernelConf` is a raw `WakuNodeConf` used as-is:
    @code{.json}
    {
    "entryLayer": "kernel",
    "kernelConf": { "clusterId": 42, "relay": true, "entryNodes": ["/dns4/…"] }
    }
    @endcode
    
    On kernel-only nodes `send` / `subscribe` / `channel*` fail with "node has
    no messaging client" / "no reliable channel manager"; `getNodeInfo`,
    `storeQuery` and metrics keep working.
    
    The pre-layered flat shape (bare `WakuNodeConf` keys at top level) still
    parses and boots the full stack.
    
    @param cfg UTF-8 JSON payload string.
    @return `true` if context creation succeeds and callback returns `RET_OK`,
    otherwise `false`.

LogosResult start()
  Signature: start()
  Invokable: yes
  Description:
    @brief Starts the delivery node.
    @return `true` once dispatched; completion is reported via `nodeStarted`.

LogosResult stop()
  Signature: stop()
  Invokable: yes
  Description:
    @brief Stops the delivery node.
    @return `true` once dispatched; completion is reported via `nodeStopped`.

LogosResult send(QString contentTopic, QByteArray payload)
  Signature: send(QString,QByteArray)
  Invokable: yes
  Description:
    @brief Sends a message over the active node.
    
    Builds a JSON envelope expected by `logosdelivery_send`:
    `{ "contentTopic": string, "payload": base64, "ephemeral": false }`.
    
    Returns a requestId on success. Async results come via typed events:
    - `messageError` emitted if the module can't send the message
    - `messagePropagated` emitted if the message has hit the network
    - `messageSent` emitted after the message is validated by the network
    
    @param contentTopic Destination content topic.
    @param payload Raw message bytes; base64-encoded before crossing the FFI boundary.
    @return Success with request id, or error details.

LogosResult subscribe(QString contentTopic)
  Signature: subscribe(QString)
  Invokable: yes
  Description:
    @brief Subscribes to the supplied content topic.
    @param contentTopic Topic identifier.
    @return `true` when subscribed successfully, otherwise `false`.

LogosResult unsubscribe(QString contentTopic)
  Signature: unsubscribe(QString)
  Invokable: yes
  Description:
    @brief Unsubscribes from the supplied content topic.
    @param contentTopic Topic identifier.
    @return `true` when unsubscribed successfully, otherwise `false`.

LogosResult storeQuery(QString jsonQuery, QString peerAddr, int timeoutMs)
  Signature: storeQuery(QString,QString,int)
  Invokable: yes
  Description:
    @brief Runs a Store (historical message) query against a specific store
    service peer.
    
    ⚠️ USE AT YOUR OWN RISK: backed by the kernel API (`waku_store_query`,
    `liblogosdelivery_kernel.h`), which is subject to change at any point
    without a deprecation cycle. This method's JSON contract follows it.
    
    The query JSON maps to logos-delivery's `StoreQueryRequest`
    (`library/kernel_api/protocols/store_api.nim`):
    | Key                 | Type            | Required | Description                                        |
    |---------------------|-----------------|----------|----------------------------------------------------|
    | `requestId`         | string          | yes      | Caller-chosen id, echoed in the response           |
    | `includeData`       | boolean         | yes      | `true` returns full messages, `false` hashes only  |
    | `paginationForward` | boolean         | yes      | Paging direction                                   |
    | `pubsubTopic`       | string          | no       | Pubsub topic filter                                |
    | `contentTopics`     | array of string | no       | Content topic filters                              |
    | `timeStart`         | number/string   | no       | Range start, nanoseconds since Unix epoch          |
    | `timeEnd`           | number/string   | no       | Range end, nanoseconds since Unix epoch            |
    | `messageHashes`     | array of string | no       | Hex message hashes for lookup-by-hash queries      |
    | `paginationCursor`  | string          | no       | Hex cursor from a previous response                |
    | `paginationLimit`   | number          | no       | Max messages per page                              |
    
    On success the result value is the response JSON (`StoreQueryResponseHex`):
    `{ "requestId", "statusCode", "statusDesc", "messages": [ { "messageHash",
    "message", "pubsubTopic" } ], "paginationCursor" }` with hashes 0x-hex
    encoded.
    
    @param jsonQuery UTF-8 JSON query document, see above.
    @param peerAddr Multiaddress of the store service peer to query
    (e.g. `/ip4/127.0.0.1/tcp/60000/p2p/16Uiu2...`).
    @param timeoutMs Query timeout in milliseconds.
    @return Success with the response JSON, or error details.

LogosResult channelCreate(QString channelId, QString contentTopic, QString senderId)
  Signature: channelCreate(QString,QString,QString)
  Invokable: yes
  Description:
    @brief Creates (or re-opens) a reliable channel.
    
    Persisted channel state survives @ref channelClose, so re-creating a
    channel with the same id restores it.
    
    @param channelId Application-chosen channel identifier.
    @param contentTopic Content topic the channel communicates on.
    @param senderId This participant's SDS (Scalable Data Sync) sender identifier.
    @return Success with the channel id, or error details.

LogosResult channelExists(QString channelId)
  Signature: channelExists(QString)
  Invokable: yes
  Description:
    @brief Checks whether a reliable channel is currently open.
    
    An unknown channel id is not an error.
    
    @param channelId Channel identifier.
    @return Success with `"true"` or `"false"` (verbatim FFI string), or error details.

LogosResult channelSend(QString channelId, QByteArray payload)
  Signature: channelSend(QString,QByteArray)
  Invokable: yes
  Description:
    @brief Sends a message on a reliable channel.
    
    Builds the JSON envelope expected by `logosdelivery_channel_send`:
    `{ "payload": base64, "ephemeral": false }`.
    
    Returns a requestId on success. Async results come via typed events:
    - `channelMessageSent` once every segment of the send is confirmed
    - `channelMessageError` if the send finalises with a failed segment
    
    @param channelId Channel identifier.
    @param payload Raw message bytes; base64-encoded before crossing the FFI boundary.
    @return Success with request id, or error details.

LogosResult channelClose(QString channelId)
  Signature: channelClose(QString)
  Invokable: yes
  Description:
    @brief Closes a reliable channel: stops its SDS loops.
    
    Persisted state survives, so @ref channelCreate with the same id
    restores the channel.
    
    @param channelId Channel identifier.
    @return `true` when closed successfully, otherwise `false`.

LogosResult getAvailableNodeInfoIDs()
  Signature: getAvailableNodeInfoIDs()
  Invokable: yes

[exit 0]
```

### S48 · PASS · lm events delivery_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/delivery_module/variants/linux-amd64-dev/delivery_module_plugin.so
Plugin Events:
==============

void messageSent(QString requestId, QString messageHash, int timestamp)
  Signature: messageSent(QString,QString,int)

void messageError(QString requestId, QString messageHash, QString error, int timestamp)
  Signature: messageError(QString,QString,QString,int)

void messagePropagated(QString requestId, QString messageHash, int timestamp)
  Signature: messagePropagated(QString,QString,int)

void messageReceived(QString messageHash, QString contentTopic, QByteArray payload, int timestamp)
  Signature: messageReceived(QString,QString,QByteArray,int)

void connectionStateChanged(QString connectionStatus, int timestamp)
  Signature: connectionStateChanged(QString,int)

void channelMessageReceived(QString channelId, QString senderId, QByteArray payload, int timestamp)
  Signature: channelMessageReceived(QString,QString,QByteArray,int)

void channelMessageSent(QString channelId, QString requestId, int timestamp)
  Signature: channelMessageSent(QString,QString,int)

void channelMessageError(QString channelId, QString requestId, QString error, int timestamp)
  Signature: channelMessageError(QString,QString,QString,int)

void nodeStarted(bool success, QString message, int timestamp)
  Signature: nodeStarted(bool,QString,int)

void nodeStopped(bool success, QString message, int timestamp)
  Signature: nodeStopped(bool,QString,int)

[exit 0]
```

### S49 · PASS · lm metadata lez_indexer_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/lez_indexer_module/variants/linux-amd64/lez_indexer_module_plugin.so
Plugin Metadata:
================
Name:         lez_indexer_module
Display name: LEZ Indexer Module
Version:      1.1.1-rpc.4
Description:  Logos Execution Zone Indexer Module for Logos Core
Author:       
Type:         core
Protocol:     0.1.0
Dependencies: (none)
[exit 0]
```

### S50 · PASS · lm methods lez_indexer_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/lez_indexer_module/variants/linux-amd64/lez_indexer_module_plugin.so
Plugin Methods:
===============

int start_indexer(QString config_path)
  Signature: start_indexer(QString)
  Invokable: yes
  Description:
    Boot ingestion against the indexer config at `config_path` (must be an
    ABSOLUTE path — the module runs in a logos_host subprocess). RocksDB state
    is stored under this module's instance persistence path (host-owned),
    independent of the config. Idempotent: a second call while already running
    is a no-op. Returns 0 on success, else the FFI OperationStatus code.
    int64_t (not int): the universal codegen marshals int64_t/std::string/bool
    as scalar wire types; a plain `int` return is treated as a JSON payload.

int stop_indexer()
  Signature: stop_indexer()
  Invokable: yes
  Description:
    Stop ingestion and release the FFI handle. No-op (returns 0) when the
    indexer isn't running. Pair with start_indexer to apply a new config:
    stop, then start (start_indexer stays idempotent and won't restart on its
    own). Returns 0 on success, else the FFI OperationStatus code.

int reset_storage(QString config_path)
  Signature: reset_storage(QString)
  Invokable: yes
  Description:
    Stop the indexer (if running) and delete the RocksDB store for the config's
    channel — `<storage>/rocksdb-<channel_id>`, where `<storage>` is resolved the
    same way start_indexer resolves it (the host's instance persistence path, or
    the process working directory when unset) — so the next start_indexer
    re-indexes from scratch. The recovery path when the store is stale against a
    different/reset chain. Pass the same `config_path` given to start_indexer;
    does NOT restart. Returns 0 on success, else non-zero. int64_t for the same
    codegen reason as start_indexer.

QString getAccount(QString account_id)
  Signature: getAccount(QString)
  Invokable: yes
  Description:
    Account by id, accepting Base58 (canonical) or 32-byte hex. The returned
    JSON omits the id; callers inject the queried id themselves.

QString getBlockById(QString block_id)
  Signature: getBlockById(QString)
  Invokable: yes
  Description: Block by decimal block id.

QString getBlockByHash(QString hash)
  Signature: getBlockByHash(QString)
  Invokable: yes
  Description: Block by 32-byte hex hash.

QString getTransaction(QString hash)
  Signature: getTransaction(QString)
  Invokable: yes
  Description: Transaction by 32-byte hex hash.

QString getBlocks(QString before, QString limit)
  Signature: getBlocks(QString,QString)
  Invokable: yes
  Description:
    Page of blocks: `before` = "" for the tip, else a block id to page back
    from; `limit` is the decimal max count.

QString getLastFinalizedBlockId()
  Signature: getLastFinalizedBlockId()
  Invokable: yes
  Description: Tip block id as a bare decimal string.

QString getStatus()
  Signature: getStatus()
  Invokable: yes
  Description:
    Current ingestion status as the core's `IndexerStatus` JSON, passed
    through verbatim: { state (Starting/Syncing/CaughtUp/Error/Stalled),
    last_error, indexed_block_id, stall_reason }. Empty string if the
    indexer isn't running.

QString getTransactionsByAccount(QString account_id, QString offset, QString limit)
  Signature: getTransactionsByAccount(QString,QString,QString)
  Invokable: yes
  Description:
    Transactions touching `account_id` (Base58 or 32-byte hex), paginated by
    decimal `offset`/`limit`.

int serve_rpc(int port)
  Signature: serve_rpc(int)
  Invokable: yes

[exit 0]
```

### S51 · PASS · lm events lez_indexer_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/lez_indexer_module/variants/linux-amd64/lez_indexer_module_plugin.so
Plugin Events:
==============

(no events found)
[exit 0]
```

### S52 · PASS · lm metadata blockchain_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm metadata /x/blockchain_module/variants/linux-amd64/blockchain_module_plugin.so
Plugin Metadata:
================
Name:         blockchain_module
Display name: Blockchain Module
Version:      0.2.4
Description:  Logos blockchain node for logos-core
Author:       Logos Blockchain Team
Type:         core
Protocol:     0.2.0
Dependencies: (none)
[exit 0]
```

### S53 · PASS · lm methods blockchain_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm methods /x/blockchain_module/variants/linux-amd64/blockchain_module_plugin.so
Plugin Methods:
===============

LogosResult start(QString config_path, QString deployment)
  Signature: start(QString,QString)
  Invokable: yes

LogosResult stop()
  Signature: stop()
  Invokable: yes

LogosResult generate_user_config(QString json_args)
  Signature: generate_user_config(QString)
  Invokable: yes

LogosResult update_user_config(QString user_config_path, QString keystore_path)
  Signature: update_user_config(QString,QString)
  Invokable: yes

LogosResult migrate_user_config(QString output_path, QString keystore_path)
  Signature: migrate_user_config(QString,QString)
  Invokable: yes

LogosResult migrate_user_config_0_1_2(QString new_config_path, QString old_config_path, QString keystore_path)
  Signature: migrate_user_config_0_1_2(QString,QString,QString)
  Invokable: yes

LogosResult participate(QString config_path, QString keystore_path, QString output_dir, QString external_address)
  Signature: participate(QString,QString,QString,QString)
  Invokable: yes

LogosResult generate_key(QString user_config_path, QString keystore_path, QString key_type, QString key_title)
  Signature: generate_key(QString,QString,QString,QString)
  Invokable: yes

LogosResult add_key(QString user_config_path, QString keystore_path, QString key_type, QString key_hex, QString key_title)
  Signature: add_key(QString,QString,QString,QString,QString)
  Invokable: yes

LogosResult remove_key(QString user_config_path, QString keystore_path, QString key_title)
  Signature: remove_key(QString,QString,QString)
  Invokable: yes

LogosResult get_peer_id(QString config_path)
  Signature: get_peer_id(QString)
  Invokable: yes

LogosResult wallet_get_balance(QString address_hex)
  Signature: wallet_get_balance(QString)
  Invokable: yes

LogosResult wallet_transfer_funds(QString change_public_key, QStringList sender_addresses, QString recipient_address, QString amount, QString optional_tip_hex)
  Signature: wallet_transfer_funds(QString,QStringList,QString,QString,QString)
  Invokable: yes

LogosResult wallet_get_known_addresses()
  Signature: wallet_get_known_addresses()
  Invokable: yes

LogosResult wallet_get_notes(QString wallet_address_hex, QString optional_tip_hex)
  Signature: wallet_get_notes(QString,QString)
  Invokable: yes

LogosResult leader_claim()
  Signature: leader_claim()
  Invokable: yes

LogosResult wallet_get_claimable_vouchers()
  Signature: wallet_get_claimable_vouchers()
  Invokable: yes

LogosResult wallet_fund_tx(QString request_json)
  Signature: wallet_fund_tx(QString)
  Invokable: yes

LogosResult submit_signed_transaction(QString signed_tx_json)
  Signature: submit_signed_transaction(QString)
  Invokable: yes

LogosResult channel_deposit(QString channel_id_hex, QString funding_public_key_hex, QString amount, QString metadata_hex, QString optional_tip_hex)
  Signature: channel_deposit(QString,QString,QString,QString,QString)
  Invokable: yes

LogosResult channel_deposit_with_notes(QString channel_id_hex, QStringList input_note_id_hexes, QString metadata_hex, QString change_public_key_hex, QStringList funding_public_key_hexes, QString max_tx_fee, QString optional_tip_hex)
  Signature: channel_deposit_with_notes(QString,QStringList,QString,QString,QStringList,QString,QString)
  Invokable: yes

LogosResult get_channel_state(QString channel_id_hex)
  Signature: get_channel_state(QString)
  Invokable: yes

LogosResult blend_join_as_core_node(QString locator, QString locked_note_id_hex)
  Signature: blend_join_as_core_node(QString,QString)
  Invokable: yes

LogosResult get_block(QString header_id_hex)
  Signature: get_block(QString)
  Invokable: yes

LogosResult get_blocks(int from_slot, int to_slot)
  Signature: get_blocks(int,int)
  Invokable: yes

LogosResult get_transaction(QString tx_hash_hex)
  Signature: get_transaction(QString)
  Invokable: yes

LogosResult get_cryptarchia_info()
  Signature: get_cryptarchia_info()
  Invokable: yes

LogosResult get_block_events(QString header_id_hex)
  Signature: get_block_events(QString)
  Invokable: yes

LogosResult get_time_info()
  Signature: get_time_info()
  Invokable: yes

[exit 0]
```

### S54 · PASS · lm events blockchain_module

```
[tools] $ LD_LIBRARY_PATH=/opt/logoscore/usr/lib /x/lm events /x/blockchain_module/variants/linux-amd64/blockchain_module_plugin.so
Plugin Events:
==============

void newBlock(QString blockJson)
  Signature: newBlock(QString)

void processedBlock(QString eventJson)
  Signature: processedBlock(QString)

void libBlock(QString blockInfoJson)
  Signature: libBlock(QString)

[exit 0]
```

### S55 · NOT RUN · lm verify <installed dir> --did <pinned signer>

```
lm 0.1.0 (the module builder's) has no verify; logos-module 0.3.0, which has it, publishes no binary (a nix build). The packages are unsigned, so there is no DID to pin either.
```

## Install the role package alone

### S56 · FAIL · install lez_atomic_swap_maker as built

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/lez_atomic_swap_maker.lgx --root-hash cdc00bd5366c84e1c323b7f31b1b80e5ba45470d5426e4c9cbd04e8c603c2422 -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_atomic_swap_maker.lgx","fromVersion":"","name":"lez_atomic_swap_maker","signature":"unsigned","toVersion":"0.2.5"}],"code":"PACKAGE_OP_FAILED","error":"Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","failed_step":"install","message":"install failed at step 'install': Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","op":"install","status":"error","stopped_dependents":[]}
[exit 1]
```

### S57 · INFO · DERIVED: lez_atomic_swap_maker with variant linux-amd64-dev renamed linux-amd64 (lgx extract/add/remove)

```
[tools] $ cp /in/lez_atomic_swap_maker.lgx /x/lez_atomic_swap_maker-linux-amd64.lgx && chmod u+w /x/lez_atomic_swap_maker-linux-amd64.lgx && /x/lgx extract /x/lez_atomic_swap_maker-linux-amd64.lgx -v linux-amd64-dev -o /x/relabel && /x/lgx add /x/lez_atomic_swap_maker-linux-amd64.lgx -v linux-amd64 -f /x/relabel/linux-amd64-dev --main lez_atomic_swap_maker_plugin.so --view qml/Main.qml -y && /x/lgx remove /x/lez_atomic_swap_maker-linux-amd64.lgx -v linux-amd64-dev -y && /x/lgx manifest /x/lez_atomic_swap_maker-linux-amd64.lgx
Extracted variant 'linux-amd64-dev' to /x/relabel
Added variant 'linux-amd64' to /x/lez_atomic_swap_maker-linux-amd64.lgx
Removed variant 'linux-amd64-dev' from /x/lez_atomic_swap_maker-linux-amd64.lgx
Name:           lez_atomic_swap_maker
Display name:   LEZ Maker
Version:        0.2.5
Manifest ver.:  0.3.0
Type:           ui_qml
Category:       finance
Author:         (unset)
Description:    Publish wallet-owned BTC/LEZ offers and perform Maker settlement actions.
Icon:           maker-console.svg
View:           qml/Main.qml
Root hash:      b4be9c11a58a74927b0d3877bfa39f46e093431affd0e02cbd91864a0edbdc3c
Variants:       linux-amd64
                linux-amd64 -> lez_atomic_swap_maker_plugin.so
Dependencies:
  - chat_module
  - delivery_module
  - lez_core
  - lez_swap_node
  - lez_maker_node
Signed:         no
[exit 0]
```

### S58 · PASS · package show (derived)

```
[run] $ logosctl --config-dir /tmp/acceptance --json package show /tmp/lez_atomic_swap_maker-linux-amd64.lgx
{"category":"finance","description":"Publish wallet-owned BTC/LEZ offers and perform Maker settlement actions.","installedHash":"","installedVersion":"","isAlreadyInstalled":false,"name":"lez_atomic_swap_maker","rootHash":"b4be9c11a58a74927b0d3877bfa39f46e093431affd0e02cbd91864a0edbdc3c","signatureStatus":"unsigned","type":"ui_qml","variants":["linux-amd64"],"version":"0.2.5"}
[exit 0]
```

### S59 · PASS · install lez_atomic_swap_maker (derived) with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /tmp/lez_atomic_swap_maker-linux-amd64.lgx --root-hash b4be9c11a58a74927b0d3877bfa39f46e093431affd0e02cbd91864a0edbdc3c -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/tmp/lez_atomic_swap_maker-linux-amd64.lgx","fromVersion":"","name":"lez_atomic_swap_maker","signature":"unsigned","toVersion":"0.2.5"}],"installed":["lez_atomic_swap_maker"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S60 · FAIL · installing the role package pulled in its dependencies

```
[run] $ logosctl --config-dir /tmp/acceptance package deps lez_atomic_swap_maker --recursive
[{"installType":"","name":"chat_module","status":"not_installed","version":""},{"installType":"","name":"delivery_module","status":"not_installed","version":""},{"installType":"","name":"lez_core","status":"not_installed","version":""},{"installType":"","name":"lez_swap_node","status":"not_installed","version":""},{"installType":"","name":"lez_maker_node","status":"not_installed","version":""}]
[exit 0]
```

### S61 · FAIL · the role package declares its LEZ network requirement

```
[run] $ logosctl --config-dir /tmp/acceptance package deps lez_atomic_swap_maker --recursive
[{"installType":"","name":"chat_module","status":"not_installed","version":""},{"installType":"","name":"delivery_module","status":"not_installed","version":""},{"installType":"","name":"lez_core","status":"not_installed","version":""},{"installType":"","name":"lez_swap_node","status":"not_installed","version":""},{"installType":"","name":"lez_maker_node","status":"not_installed","version":""}]
[exit 0]
```

## Install the modules

### S62 · PASS · install lez_core with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/lez_core.lgx --root-hash 8872f8e34689b5ae0953423b57a2b317ed748fd03a4c46e232049d182c792edf -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_core.lgx","fromVersion":"","name":"lez_core","signature":"unsigned","toVersion":"0.4.1-sign.4"}],"installed":["lez_core"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S63 · PASS · install lez_swap_node with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/lez_swap_node.lgx --root-hash a9e04c9f7dca3c7a18163ed7380a70e5b40e3ac83b280ed5248de9edd40da104 -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_swap_node.lgx","fromVersion":"","name":"lez_swap_node","signature":"unsigned","toVersion":"0.3.0"}],"installed":["lez_swap_node"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S64 · PASS · install lez_maker_node with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/lez_maker_node.lgx --root-hash 939e318ea2ab413da35520d2f179916ea3e4bab920e722658d96fb3fc0e0d99e -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_maker_node.lgx","fromVersion":"","name":"lez_maker_node","signature":"unsigned","toVersion":"0.1.0"}],"installed":["lez_maker_node"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S65 · FAIL · install chat_module with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/chat_module.lgx --root-hash e4c0346cf47d420d9a05a8f9a1f0867d232c2546c9c7102df86c66c739b5a499 -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/chat_module.lgx","fromVersion":"","name":"chat_module","signature":"unsigned","toVersion":"0.2.2"}],"code":"PACKAGE_OP_FAILED","error":"Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","failed_step":"install","message":"install failed at step 'install': Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","op":"install","status":"error","stopped_dependents":[]}
[exit 1]
```

### S66 · FAIL · install delivery_module with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/delivery_module.lgx --root-hash 36eaa5da4e5f47bf111fd39e1ea42bba67f69f1018177532fa2e97b87bf8562f -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/delivery_module.lgx","fromVersion":"","name":"delivery_module","signature":"unsigned","toVersion":"0.2.0"}],"code":"PACKAGE_OP_FAILED","error":"Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","failed_step":"install","message":"install failed at step 'install': Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","op":"install","status":"error","stopped_dependents":[]}
[exit 1]
```

### S67 · PASS · install lez_indexer_module with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/lez_indexer_module.lgx --root-hash 6288c56b37037a6875723e60a449a4f6d31036340ecaa224ecc9b50f7d0e187d -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_indexer_module.lgx","fromVersion":"","name":"lez_indexer_module","signature":"unsigned","toVersion":"1.1.1-rpc.4"}],"installed":["lez_indexer_module"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S68 · PASS · install blockchain_module with its root hash

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/blockchain_module.lgx --root-hash 2e57268c4ec1fdcf07e4b6bf1b33b5ac99705c071f879e6ca1c41b4e543cc674 -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/blockchain_module.lgx","fromVersion":"","name":"blockchain_module","signature":"unsigned","toVersion":"0.2.4"}],"installed":["blockchain_module"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S69 · FAIL · a wrong --root-hash is refused

```
[run] $ logosctl --config-dir /tmp/acceptance package install /in/lez_swap_node.lgx --root-hash 0000000000000000000000000000000000000000000000000000000000000000 -y
{"affected_loaded":[],"changes":[{"action":"reinstall","file":"/in/lez_swap_node.lgx","fromVersion":"0.3.0","name":"lez_swap_node","signature":"unsigned","toVersion":"0.3.0"}],"installed":["lez_swap_node"],"op":"install","reloaded":[],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S70 · FAIL · every dependency of lez_atomic_swap_maker is installed

```
[run] $ logosctl --config-dir /tmp/acceptance package deps lez_atomic_swap_maker --recursive
[{"installType":"","name":"chat_module","status":"not_installed","version":""},{"installType":"","name":"delivery_module","status":"not_installed","version":""},{"installType":"user","name":"lez_core","status":"installed","version":"0.4.1-sign.4"},{"installType":"user","name":"lez_swap_node","status":"installed","version":"0.3.0"},{"installType":"user","name":"lez_maker_node","status":"installed","version":"0.1.0"}]
[exit 0]
```

### S71 · PASS · package ls

```
[run] $ logosctl --config-dir /tmp/acceptance package ls
[{"author":"Logos Blockchain Team","category":"blockchain","dependencies":[],"description":"Logos blockchain node for logos-core","displayName":"Blockchain Module","hashes":{"root":"2e57268c4ec1fdcf07e4b6bf1b33b5ac99705c071f879e6ca1c41b4e543cc674"},"icon":"","installDir":"/tmp/acceptance/modules/blockchain_module","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/blockchain_module/blockchain_module_plugin.so","manifestVersion":"0.3.0","name":"blockchain_module","source":"","type":"core","version":"0.2.4","view":""},{"author":"Logos Core Team","category":"security","dependencies":[],"description":"Coordinates permissions between modules","displayName":"Capabilities","hashes":{"root":"7a5c036dd3a66db7c8fbb7ceabe5f0dd431cf8ad8067d81457b754c02660f466"},"icon":"","installDir":"/opt/logosctl/usr/modules/capability_module","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules/capability_module/capability_module_plugin.so","manifestVersion":"0.6.0","name":"capability_module","source":"","type":"core","version":"1.0.0","view":""},{"author":"","category":"finance","dependencies":["chat_module","delivery_module","lez_core","lez_swap_node","lez_maker_node"],"description":"Publish wallet-owned BTC/LEZ offers and perform Maker settlement actions.","displayName":"LEZ Maker","hashes":{"root":"b4be9c11a58a74927b0d3877bfa39f46e093431affd0e02cbd91864a0edbdc3c"},"icon":"maker-console.svg","installDir":"/tmp/acceptance/plugins/lez_atomic_swap_maker","installType":"user","license":"","mainFilePath":"/tmp/acceptance/plugins/lez_atomic_swap_maker/lez_atomic_swap_maker_plugin.so","manifestVersion":"0.3.0","name":"lez_atomic_swap_maker","source":"","type":"ui_qml","version":"0.2.5","view":"qml/Main.qml"},{"author":"Logos Blockchain Team","category":"blockchain","dependencies":[],"description":"Logos Execution Zone Core Module for Logos Core","displayName":"LEZ Core Module","hashes":{"root":"8872f8e34689b5ae0953423b57a2b317ed748fd03a4c46e232049d182c792edf"},"icon":"","installDir":"/tmp/acceptance/modules/lez_core","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/lez_core/lez_core_plugin.so","manifestVersion":"0.3.0","name":"lez_core","source":"","type":"core","version":"0.4.1-sign.4","view":""},{"author":"","category":"blockchain","dependencies":[],"description":"Logos Execution Zone Indexer Module for Logos Core","displayName":"","hashes":{"root":"6288c56b37037a6875723e60a449a4f6d31036340ecaa224ecc9b50f7d0e187d"},"icon":"","installDir":"/tmp/acceptance/modules/lez_indexer_module","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/lez_indexer_module/lez_indexer_module_plugin.so","manifestVersion":"0.2.0","name":"lez_indexer_module","source":"","type":"core","version":"1.1.1-rpc.4","view":""},{"author":"","category":"blockchain","dependencies":["lez_swap_node"],"description":"The Maker's Node (the swap engine) and everything it runs: the Maker Node, its CLI, its Bitcoin and Zcash actors and the LEZ v0.2 sidecar, with the launcher lez_swap_node starts it from. Linux only (linux-amd64, linux-arm64)","displayName":"","hashes":{"root":"939e318ea2ab413da35520d2f179916ea3e4bab920e722658d96fb3fc0e0d99e"},"icon":"","installDir":"/tmp/acceptance/modules/lez_maker_node","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/lez_maker_node/lez_maker_node_plugin.so","manifestVersion":"0.2.0","name":"lez_maker_node","source":"","type":"core","version":"0.1.0","view":""},{"author":"","category":"blockchain","dependencies":["lez_core"],"description":"Launches and supervises a swap role's Node (the swap engine) inside Basecamp and relays its LEZ signing requests to Basecamp's wallet","displayName":"","hashes":{"root":"a9e04c9f7dca3c7a18163ed7380a70e5b40e3ac83b280ed5248de9edd40da104"},"icon":"","installDir":"/tmp/acceptance/modules/lez_swap_node","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/lez_swap_node/lez_swap_node_plugin.so","manifestVersion":"0.2.0","name":"lez_swap_node","source":"","type":"core","version":"0.3.0","view":""},{"author":"Logos Core Team","category":"system","dependencies":[],"description":"Read-only registry of module lifecycle state","displayName":"Module State","hashes":{"root":"7447a632ae0fd8941c21bb277606ad178c3a24658ca4a31eb004719b47e4bf12"},"icon":"","installDir":"/opt/logosctl/usr/modules/modules_state","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules/modules_state/modules_state_plugin.so","manifestVersion":"0.6.0","name":"modules_state","source":"","type":"core","version":"0.1.0","view":""},{"author":"Logos Core Team","category":"management","dependencies":[],"description":"Online package catalog and download service","displayName":"Package Downloader","hashes":{"root":"7e9dde67c2b24c82c57064dabab8b9a3bed3853549574ba17348f84713713ba3"},"icon":"","installDir":"/opt/logosctl/usr/modules-pkg/package_downloader","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules-pkg/package_downloader/package_downloader_plugin.so","manifestVersion":"0.6.0","name":"package_downloader","optionalDependencies":["storage_module","modules_state"],"source":"","type":"core","version":"1.0.0","view":""},{"author":"Logos Core Team","category":"management","dependencies":[],"description":"Plugin manager for the Logos system","displayName":"Package Manager","hashes":{"root":"493ad00f3020482453fd17db2a339881b1784a6cba213bffe4c25749beca2318"},"icon":"","installDir":"/opt/logosctl/usr/modules-pkg/package_manager","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules-pkg/package_manager/package_manager_plugin.so","manifestVersion":"0.6.0","name":"package_manager","source":"","type":"core","version":"1.0.0","view":""},{"author":"Logos Core Team","category":"protocol","dependencies":[],"description":"Storage module","displayName":"Storage Module","hashes":{"root":"43e40ec04039e2d258fafa4383680aa51f0514d0542d08660a708d919818a8d2"},"icon":"","installDir":"/opt/logosctl/usr/modules-pkg/storage_module","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules-pkg/storage_module/storage_module_plugin.so","manifestVersion":"0.6.0","name":"storage_module","source":"","type":"core","version":"3.0.2","view":""}]
[exit 0]
```

## Lifecycle and behaviour

### S72 · FAIL · module load lez_atomic_swap_maker

```
[run] $ logosctl --config-dir /tmp/acceptance module load lez_atomic_swap_maker
{"code":"MODULE_LOAD_FAILED","known_modules":["blockchain_module","lez_maker_node","lez_swap_node","lez_core","package_manager","storage_module","package_downloader","modules_state","lez_indexer_module","capability_module"],"message":"Failed to load module 'lez_atomic_swap_maker'.","status":"error"}
[exit 3]
```

### S73 · PASS · module load lez_swap_node (with its declared dependencies)

```
[run] $ logosctl --config-dir /tmp/acceptance module load lez_swap_node
{"dependencies_loaded":["lez_core"],"module":"lez_swap_node","status":"ok","version":"0.3.0"}
[exit 0]
```

### S74 · PASS · module show lez_swap_node

```
[run] $ logosctl --config-dir /tmp/acceptance module show lez_swap_node
{"dependencies":["lez_core"],"dependents":["lez_maker_node"],"events":[],"methods":[{"description":"Start supervising `role`'s Node: `launcher` (absolute, executable) run\nwith `role` as its one argument, output appended to `log_file`, held\ndown while `hold_file` exists. Idempotent: a role already supervised is\nleft as it is. Returns 0 when supervised, else non-zero (1 invalid\narguments). int64_t: the universal codegen marshals it as a scalar.","isInvokable":true,"name":"start_node","parameters":[{"name":"role","type":"QString"},{"name":"launcher","type":"QString"},{"name":"log_file","type":"QString"},{"name":"hold_file","type":"QString"}],"returnType":"int","signature":"start_node(QString,QString,QString,QString)"},{"description":"Stop `role`'s Node (SIGTERM to its process group, SIGKILL after 10 s),\nstop supervising it and forget it (a reload does not start it again).\nReturns 0, also for a role not supervised.","isInvokable":true,"name":"stop_node","parameters":[{"name":"role","type":"QString"}],"returnType":"int","signature":"stop_node(QString)"},{"description":"`role`'s supervision as compact JSON: {role, state (running, held,\nrestarting, stopped), pid, starts, last_exit}; empty when the role was\nnever started.","isInvokable":true,"name":"node_status","parameters":[{"name":"role","type":"QString"}],"returnType":"QString","signature":"node_status(QString)"},{"description":"Serve `account_id_hex`'s LEZ signatures to the role's Node on the\nowner-only Unix socket `socket_path`: one JSON line per connection,\n{\"account_id\", \"message_hash\"} in, the wallet's {\"signature\",\n\"public_key\"} (or {\"error\"}) out. Every signature is made by Basecamp's\nLEZ wallet (lez_core, sign_public_message_hash); the key never reaches\nthis module or the Node. Only that one account is served, and a socket\nalready served is left as it is. Returns 0 when serving, else non-zero.","isInvokable":true,"name":"serve_signer","parameters":[{"name":"socket_path","type":"QString"},{"name":"account_id_hex","type":"QString"}],"returnType":"int","signature":"serve_signer(QString,QString)"},{"description":"The module's name, as declared in its metadata.","isInvokable":true,"name":"name","returnType":"QString","signature":"name()","type":"method"},{"description":"The module's version, as declared in its metadata.","isInvokable":true,"name":"version","returnType":"QString","signature":"version()","type":"method"}],"name":"lez_swap_node","status":"loaded","uptime_seconds":1,"version":"0.3.0"}
[exit 0]
```

### S75 · PASS · call lez_swap_node node_status maker

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_swap_node node_status maker
{"method":"node_status","module":"lez_swap_node","result":"","status":"ok"}
[exit 0]
```

### S76 · PASS · the installed package graph carries the maker Node and its launcher

```
[run] $ find /tmp/acceptance/modules /tmp/acceptance/plugins \( -name node-entrypoint.sh -o -name 'lez-maker-node*' \) -print
/tmp/acceptance/modules/lez_maker_node/bin/node-entrypoint.sh
/tmp/acceptance/modules/lez_maker_node/bin/lez-maker-node
[exit 0]
```

### S77 · PASS · module load lez_maker_node (the maker Node package, with its declared dependencies)

```
[run] $ logosctl --config-dir /tmp/acceptance module load lez_maker_node
{"dependencies_loaded":[],"module":"lez_maker_node","status":"ok","version":"0.1.0"}
[exit 0]
```

### S78 · PASS · call lez_maker_node launcher: the installed launcher

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_maker_node launcher
{"method":"launcher","module":"lez_maker_node","result":"/tmp/acceptance/modules/lez_maker_node/bin/node-entrypoint.sh","status":"ok"}
[exit 0]
```

### S79 · PASS · call lez_maker_node payload: the installed files and their sha256

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_maker_node payload
{"method":"payload","module":"lez_maker_node","result":"{\n  \"schema_version\": 1,\n  \"role\": \"maker\",\n  \"build_source\": \"52838ce1a12503e0237296e0533b3fac341faf86\",\n  \"sidecar_source\": \"2fcae45760e2e62341c86dff2931d0fc742ebe44\",\n  \"sidecar_tree\": \"f97224728ea07b176e4ec369cebf207bb7ee3dba\",\n  \"variant\": \"linux-amd64\",\n  \"files\": {\n    \"lez-btc-maker-actor\": \"174bb310ddc896b86089176591e3f9226c272b5fb1824417c297a87e6991800e\",\n    \"lez-maker-cli\": \"6b7aeb226ccdbf309565edaee7352add52150f5bc111098627886364099d3dc2\",\n    \"lez-maker-node\": \"491ceeab8c36d6d9948bec0b5448afe7b8637c634b1484712262d45a1a8fe4a2\",\n    \"lez-runtime-healthcheck\": \"d91d816d0f1b722cb9b61182f90567f1667e1d2391e8e9d75f5fe18e0be29f19\",\n    \"lez-v02-bridge-poc\": \"2d87dfcca8399c523d1456cda5e4ee426fa58568b4b8382d61523d7c38077afc\",\n    \"lez-zec-maker-actor\": \"f32a5e3404336c6199349b4b86680718d3e2ccd89564259366742ab5f7749ce7\",\n    \"node-entrypoint.sh\": \"36d56f0d02cc8cc21253ed34250167b53f6ef3e4b882fe684e073167f59a0c9d\"\n  }\n}\n","status":"ok"}
[exit 0]
```

### S80 · PASS · start_node with the package's launcher

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_swap_node start_node maker /tmp/acceptance/modules/lez_maker_node/bin/node-entrypoint.sh /tmp/acceptance/logs/maker-node.log /tmp/acceptance/maker-node.hold
{"method":"start_node","module":"lez_swap_node","result":0,"status":"ok"}
[exit 0]
```

### S81 · PASS · lez_swap_node launched the maker Node from the package

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_swap_node node_status maker
{"method":"node_status","module":"lez_swap_node","result":"{\"role\":\"maker\",\"state\":\"restarting\",\"pid\":0,\"starts\":2,\"last_exit\":\"exit 1\"}","status":"ok"}
[exit 0]
```

### S82 · INFO · the maker Node's output (without the role's configuration, which Compose provides, it stops at once)

```
[run] $ tail -5 /tmp/acceptance/logs/maker-node.log
/tmp/acceptance/modules/lez_maker_node/bin/node-entrypoint.sh: line 30: LEZ_SIDECAR_PORT_BASE: parameter null or not set
/tmp/acceptance/modules/lez_maker_node/bin/node-entrypoint.sh: line 30: LEZ_SIDECAR_PORT_BASE: parameter null or not set
[exit 0]
```

### S83 · INFO · STUB: a stand-in launcher (sleeps; not a Node)

```
[run] $ printf '#!/bin/sh\necho stub $1 $$\nexec sleep 300\n' > /tmp/stub-launcher && chmod 0755 /tmp/stub-launcher && cat /tmp/stub-launcher
#!/bin/sh
echo stub $1 $$
exec sleep 300
[exit 0]
```

### S84 · PASS · STUB: start_node with the stand-in launcher

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_swap_node start_node maker /tmp/stub-launcher /tmp/stub.log /tmp/stub.hold
{"method":"start_node","module":"lez_swap_node","result":0,"status":"ok"}
[exit 0]
```

### S85 · PASS · STUB: node_status reports the supervised child

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_swap_node node_status maker
{"method":"node_status","module":"lez_swap_node","result":"{\"role\":\"maker\",\"state\":\"running\",\"pid\":358,\"starts\":1,\"last_exit\":\"\"}","status":"ok"}
[exit 0]
```

### S86 · PASS · module stats

```
[run] $ logosctl --config-dir /tmp/acceptance module stats
[{"cpu_percent":0.0,"cpu_time_seconds":0.02,"memory_mb":22.36328125,"name":"lez_maker_node","pid":288},{"cpu_percent":0.0,"cpu_time_seconds":0.03,"memory_mb":22.78125,"name":"lez_swap_node","pid":256},{"cpu_percent":0.0,"cpu_time_seconds":0.02,"memory_mb":24.26171875,"name":"lez_core","pid":254},{"cpu_percent":0.0,"cpu_time_seconds":0.04,"memory_mb":25.03515625,"name":"package_downloader","pid":35},{"cpu_percent":0.0,"cpu_time_seconds":0.11,"memory_mb":40.875,"name":"storage_module","pid":33},{"cpu_percent":0.0,"cpu_time_seconds":59.78,"memory_mb":68.05859375,"name":"package_manager","pid":31},{"cpu_percent":0.0,"cpu_time_seconds":0.02,"memory_mb":22.86328125,"name":"modules_state","pid":29},{"cpu_percent":0.0,"cpu_time_seconds":0.03,"memory_mb":23.515625,"name":"capability_module","pid":27}]
[exit 0]
```

### S87 · FAIL · module stats shows the maker Node (pid 358) itself

```
[run] $ logosctl --config-dir /tmp/acceptance module stats
[{"cpu_percent":0.0,"cpu_time_seconds":0.02,"memory_mb":22.36328125,"name":"lez_maker_node","pid":288},{"cpu_percent":0.0,"cpu_time_seconds":0.03,"memory_mb":22.78125,"name":"lez_swap_node","pid":256},{"cpu_percent":0.0,"cpu_time_seconds":0.02,"memory_mb":24.26171875,"name":"lez_core","pid":254},{"cpu_percent":0.0,"cpu_time_seconds":0.04,"memory_mb":25.03515625,"name":"package_downloader","pid":35},{"cpu_percent":0.0,"cpu_time_seconds":0.11,"memory_mb":40.875,"name":"storage_module","pid":33},{"cpu_percent":0.0,"cpu_time_seconds":59.78,"memory_mb":68.05859375,"name":"package_manager","pid":31},{"cpu_percent":0.0,"cpu_time_seconds":0.02,"memory_mb":22.86328125,"name":"modules_state","pid":29},{"cpu_percent":6.457451772779735,"cpu_time_seconds":0.04,"memory_mb":23.515625,"name":"capability_module","pid":27}]
[exit 0]
```

### S88 · PASS · module reload lez_swap_node

```
[run] $ logosctl --config-dir /tmp/acceptance module reload lez_swap_node
{"action":"reload","module":"lez_swap_node","previous_status":"loaded","status":"loaded","version":"0.3.0"}
[exit 0]
```

### S89 · PASS · the maker Node is supervised again after the reload

```
[run] $ logosctl --config-dir /tmp/acceptance call lez_swap_node node_status maker
{"method":"node_status","module":"lez_swap_node","result":"{\"role\":\"maker\",\"state\":\"running\",\"pid\":394,\"starts\":1,\"last_exit\":\"\"}","status":"ok"}
[exit 0]
```

### S90 · INFO · processes after the reload

```
[run] $ procs | grep -E '^[0-9]+ [0-9]+ [^ ]*logos_host --name lez_swap_node|^[0-9]+ [0-9]+ sleep 300' || true
391 23 /opt/logosctl/usr/bin/logos_host --name lez_swap_node --path /tmp/acceptance/modules/lez_swap_node/lez_swap_node_plugin.so --instance-persistence-path /tmp/acceptance/data/lez_swap_node/64342a24a72a --token-source stdin 
394 391 sleep 300 
[exit 0]
```

### S91 · INFO · kill -9 the lez_swap_node host process

```
[run] $ pid=$(procs | awk '$3 ~ /logos_host$/ && $5 == "lez_swap_node" {print $1}'); echo host $pid; kill -9 $pid
host 391
[exit 0]
```

### S92 · FAIL · the runtime reports lez_swap_node as crashed

```
[run] $ logosctl --config-dir /tmp/acceptance module ls
[{"name":"blockchain_module","status":"not_loaded","version":"0.2.4"},{"name":"lez_maker_node","status":"loaded","uptime_seconds":16,"version":"0.1.0"},{"name":"lez_swap_node","status":"not_loaded","version":"0.3.0"},{"name":"lez_core","status":"loaded","uptime_seconds":17,"version":"0.4.1-sign.4"},{"name":"package_manager","status":"loaded","uptime_seconds":150,"version":"1.0.0"},{"name":"storage_module","status":"loaded","uptime_seconds":150,"version":"3.0.2"},{"name":"package_downloader","status":"loaded","uptime_seconds":150,"version":"1.0.0"},{"name":"modules_state","status":"loaded","uptime_seconds":150,"version":"0.1.0"},{"name":"lez_indexer_module","status":"not_loaded","version":"1.1.1-rpc.4"},{"name":"capability_module","status":"loaded","uptime_seconds":150,"version":"1.0.0"}]
[exit 0]
```

### S93 · PASS · no orphaned maker Node after the host crash

```
[run] $ procs | grep -E '^[0-9]+ [0-9]+ sleep 300' || true
[exit 0]
```

### S94 · INFO · load lez_swap_node again and start the role again (what a desk does)

```
[run] $ logosctl --config-dir /tmp/acceptance module load lez_swap_node && logosctl --config-dir /tmp/acceptance call lez_swap_node start_node maker /tmp/stub-launcher /tmp/stub.log /tmp/stub.hold
{"dependencies_loaded":[],"module":"lez_swap_node","status":"ok","version":"0.3.0"}
{"method":"start_node","module":"lez_swap_node","result":0,"status":"ok"}
[exit 0]
```

### S95 · PASS · exactly one maker Node after recovery

```
[run] $ procs | grep -E '^[0-9]+ [0-9]+ sleep 300' || true
538 535 sleep 300 
[exit 0]
```

## Remove

### S96 · PASS · package remove lez_atomic_swap_maker

```
[run] $ logosctl --config-dir /tmp/acceptance package remove lez_atomic_swap_maker -y
{"affected_loaded":[],"changes":[{"action":"remove","fromVersion":"0.2.5","name":"lez_atomic_swap_maker","toVersion":""}],"op":"remove","reloaded":[],"removed":["lez_atomic_swap_maker"],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S97 · PASS · package remove lez_core (takes its dependents)

```
[run] $ logosctl --config-dir /tmp/acceptance package remove lez_core -y
{"affected_loaded":["lez_swap_node","lez_maker_node","lez_core"],"changes":[{"action":"remove","fromVersion":"0.3.0","name":"lez_swap_node","toVersion":""},{"action":"remove","fromVersion":"0.1.0","name":"lez_maker_node","toVersion":""},{"action":"remove","fromVersion":"0.4.1-sign.4","name":"lez_core","toVersion":""}],"op":"remove","reloaded":[],"removed":["lez_swap_node","lez_maker_node","lez_core"],"status":"ok","stopped_dependents":[]}
[exit 0]
```

### S98 · PASS · nothing of ours is left installed

```
[run] $ logosctl --config-dir /tmp/acceptance package ls
[{"author":"Logos Blockchain Team","category":"blockchain","dependencies":[],"description":"Logos blockchain node for logos-core","displayName":"Blockchain Module","hashes":{"root":"2e57268c4ec1fdcf07e4b6bf1b33b5ac99705c071f879e6ca1c41b4e543cc674"},"icon":"","installDir":"/tmp/acceptance/modules/blockchain_module","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/blockchain_module/blockchain_module_plugin.so","manifestVersion":"0.3.0","name":"blockchain_module","source":"","type":"core","version":"0.2.4","view":""},{"author":"Logos Core Team","category":"security","dependencies":[],"description":"Coordinates permissions between modules","displayName":"Capabilities","hashes":{"root":"7a5c036dd3a66db7c8fbb7ceabe5f0dd431cf8ad8067d81457b754c02660f466"},"icon":"","installDir":"/opt/logosctl/usr/modules/capability_module","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules/capability_module/capability_module_plugin.so","manifestVersion":"0.6.0","name":"capability_module","source":"","type":"core","version":"1.0.0","view":""},{"author":"","category":"blockchain","dependencies":[],"description":"Logos Execution Zone Indexer Module for Logos Core","displayName":"","hashes":{"root":"6288c56b37037a6875723e60a449a4f6d31036340ecaa224ecc9b50f7d0e187d"},"icon":"","installDir":"/tmp/acceptance/modules/lez_indexer_module","installType":"user","license":"","mainFilePath":"/tmp/acceptance/modules/lez_indexer_module/lez_indexer_module_plugin.so","manifestVersion":"0.2.0","name":"lez_indexer_module","source":"","type":"core","version":"1.1.1-rpc.4","view":""},{"author":"Logos Core Team","category":"system","dependencies":[],"description":"Read-only registry of module lifecycle state","displayName":"Module State","hashes":{"root":"7447a632ae0fd8941c21bb277606ad178c3a24658ca4a31eb004719b47e4bf12"},"icon":"","installDir":"/opt/logosctl/usr/modules/modules_state","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules/modules_state/modules_state_plugin.so","manifestVersion":"0.6.0","name":"modules_state","source":"","type":"core","version":"0.1.0","view":""},{"author":"Logos Core Team","category":"management","dependencies":[],"description":"Online package catalog and download service","displayName":"Package Downloader","hashes":{"root":"7e9dde67c2b24c82c57064dabab8b9a3bed3853549574ba17348f84713713ba3"},"icon":"","installDir":"/opt/logosctl/usr/modules-pkg/package_downloader","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules-pkg/package_downloader/package_downloader_plugin.so","manifestVersion":"0.6.0","name":"package_downloader","optionalDependencies":["storage_module","modules_state"],"source":"","type":"core","version":"1.0.0","view":""},{"author":"Logos Core Team","category":"management","dependencies":[],"description":"Plugin manager for the Logos system","displayName":"Package Manager","hashes":{"root":"493ad00f3020482453fd17db2a339881b1784a6cba213bffe4c25749beca2318"},"icon":"","installDir":"/opt/logosctl/usr/modules-pkg/package_manager","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules-pkg/package_manager/package_manager_plugin.so","manifestVersion":"0.6.0","name":"package_manager","source":"","type":"core","version":"1.0.0","view":""},{"author":"Logos Core Team","category":"protocol","dependencies":[],"description":"Storage module","displayName":"Storage Module","hashes":{"root":"43e40ec04039e2d258fafa4383680aa51f0514d0542d08660a708d919818a8d2"},"icon":"","installDir":"/opt/logosctl/usr/modules-pkg/storage_module","installType":"embedded","license":"","mainFilePath":"/opt/logosctl/usr/modules-pkg/storage_module/storage_module_plugin.so","manifestVersion":"0.6.0","name":"storage_module","source":"","type":"core","version":"3.0.2","view":""}]
[exit 0]
```

### S99 · INFO · what remove leaves in the session

```
[run] $ find /tmp/acceptance/modules /tmp/acceptance/plugins /tmp/acceptance/data -maxdepth 2 | sort
/tmp/acceptance/data
/tmp/acceptance/data/capability_module
/tmp/acceptance/data/capability_module/f784f84c5542
/tmp/acceptance/data/lez_core
/tmp/acceptance/data/lez_core/538283fef4b4
/tmp/acceptance/data/lez_maker_node
/tmp/acceptance/data/lez_maker_node/3caf375aa473
/tmp/acceptance/data/lez_swap_node
/tmp/acceptance/data/lez_swap_node/64342a24a72a
/tmp/acceptance/data/modules_state
/tmp/acceptance/data/modules_state/7d291ff73012
/tmp/acceptance/data/package_downloader
/tmp/acceptance/data/package_downloader/83c5dd5f8bf4
/tmp/acceptance/data/package_manager
/tmp/acceptance/data/package_manager/c23e764a9a80
/tmp/acceptance/data/storage_module
/tmp/acceptance/data/storage_module/759bd690fbf5
/tmp/acceptance/modules
/tmp/acceptance/modules/blockchain_module
/tmp/acceptance/modules/blockchain_module/blockchain_module_plugin.so
/tmp/acceptance/modules/blockchain_module/libboost_system.so.1.87.0
/tmp/acceptance/modules/blockchain_module/libcrypto.so.3
/tmp/acceptance/modules/blockchain_module/liblogos_blockchain.so
/tmp/acceptance/modules/blockchain_module/libssl.so.3
/tmp/acceptance/modules/blockchain_module/manifest.json
/tmp/acceptance/modules/blockchain_module/variant
/tmp/acceptance/modules/lez_indexer_module
/tmp/acceptance/modules/lez_indexer_module/lez_indexer_module_plugin.so
/tmp/acceptance/modules/lez_indexer_module/libboost_system.so.1.87.0
/tmp/acceptance/modules/lez_indexer_module/libcrypto.so.3
/tmp/acceptance/modules/lez_indexer_module/libindexer_ffi.so
/tmp/acceptance/modules/lez_indexer_module/libssl.so.3
/tmp/acceptance/modules/lez_indexer_module/manifest.json
/tmp/acceptance/modules/lez_indexer_module/variant
/tmp/acceptance/plugins
[exit 0]
```

### S100 · PASS · daemon stop

```
[run] $ logosctl --config-dir /tmp/acceptance daemon stop
{"message":"Daemon shutting down.","status":"ok"}
[exit 0]
```

## Which tools take our packages

### S101 · PASS · logosctl 0.3.0: install, load lez_swap_node, call node_status

```
[compat-0.3.0] $ logosctl --config-dir /tmp/acceptance daemon start --detach >/dev/null && logosctl --config-dir /tmp/acceptance --version | head -1 && logosctl --config-dir /tmp/acceptance package install /in/lez_core.lgx -y && logosctl --config-dir /tmp/acceptance package install /in/lez_swap_node.lgx -y && logosctl --config-dir /tmp/acceptance module load lez_swap_node && logosctl --config-dir /tmp/acceptance call lez_swap_node node_status maker
logosctl version 0.3.0
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_core.lgx","fromVersion":"","name":"lez_core","signature":"unsigned","toVersion":"0.4.1-sign.4"}],"installed":["lez_core"],"op":"install","reloaded":[],"status":"ok"}
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_swap_node.lgx","fromVersion":"","name":"lez_swap_node","signature":"unsigned","toVersion":"0.3.0"}],"installed":["lez_swap_node"],"op":"install","reloaded":[],"status":"ok"}
{"dependencies_loaded":["lez_core"],"module":"lez_swap_node","status":"ok","version":"0.3.0"}
{"method":"node_status","module":"lez_swap_node","result":"","status":"ok"}
[exit 0]
```

### S102 · FAIL · logosctl 0.3.0: install lez_atomic_swap_maker as built

```
[compat-0.3.0] $ logosctl --config-dir /tmp/acceptance package install /in/lez_atomic_swap_maker.lgx -y
{"affected_loaded":[],"changes":[{"action":"install","file":"/in/lez_atomic_swap_maker.lgx","fromVersion":"","name":"lez_atomic_swap_maker","signature":"unsigned","toVersion":"0.2.5"}],"code":"PACKAGE_OP_FAILED","error":"Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","failed_step":"install","message":"install failed at step 'install': Package does not contain variant for platform: linux-x86_64 (package provides: linux-amd64-dev)","op":"install","status":"error"}
[exit 1]
```

### S103 · PASS · logoscore 0.2.3: modules laid out as the images do (no installer in this release)

```
[tools] $ mkdir -p /x/era/modules && mkdir -p /x/era/lez_core.x /x/era/modules/lez_core && tar -xzf /in/lez_core.lgx -C /x/era/lez_core.x && cp /x/era/lez_core.x/manifest.json /x/era/modules/lez_core/ && cp -r /x/era/lez_core.x/variants/linux-*/. /x/era/modules/lez_core/ && ls /x/era/lez_core.x/variants > /x/era/modules/lez_core/variant && mkdir -p /x/era/lez_swap_node.x /x/era/modules/lez_swap_node && tar -xzf /in/lez_swap_node.lgx -C /x/era/lez_swap_node.x && cp /x/era/lez_swap_node.x/manifest.json /x/era/modules/lez_swap_node/ && cp -r /x/era/lez_swap_node.x/variants/linux-*/. /x/era/modules/lez_swap_node/ && ls /x/era/lez_swap_node.x/variants > /x/era/modules/lez_swap_node/variant && ls /x/era/modules
lez_core
lez_swap_node
[exit 0]
```

### S104 · PASS · logoscore 0.2.3: load-module lez_swap_node, call node_status

```
[tools] $ logoscore --config-dir /x/era/cfg load-module lez_swap_node && logoscore --config-dir /x/era/cfg call lez_swap_node node_status maker
{"dependencies_loaded":["lez_core"],"module":"lez_swap_node","status":"ok","version":"0.3.0"}
{"method":"node_status","module":"lez_swap_node","result":"","status":"ok"}
[exit 0]
```

### S105 · FAIL · lgpm (v0.2-era, the desks' installer): install lez_swap_node

```
[tools] $ mkdir -p /x/lgpm-root/modules /x/lgpm-root/plugins && /x/lgpm --modules-dir /x/lgpm-root/modules --ui-plugins-dir /x/lgpm-root/plugins install --file /in/lez_swap_node.lgx
Installing from file: /in/lez_swap_node.lgx...Warning: Package is unsigned: /in/lez_swap_node.lgx
Warning: package '/in/lez_swap_node.lgx' has no variant matching this platform: tried [linux-x86_64-dev, linux-amd64-dev], package provides [linux-amd64].
 FAILED
Error: Package does not contain variant for platform: linux-x86_64-dev (package provides: linux-amd64)
[exit 1]
```

### S106 · PASS · lgpm (v0.2-era, the desks' installer): install lez_atomic_swap_maker

```
[tools] $ mkdir -p /x/lgpm-root/modules /x/lgpm-root/plugins && /x/lgpm --modules-dir /x/lgpm-root/modules --ui-plugins-dir /x/lgpm-root/plugins install --file /in/lez_atomic_swap_maker.lgx
Installing from file: /in/lez_atomic_swap_maker.lgx...Warning: Package is unsigned: /in/lez_atomic_swap_maker.lgx
 done
Installed to: /x/lgpm-root/plugins
[exit 0]
```

## Summary

```
S01	PASS	logosctl version
S02	PASS	lm version (v0.2-era, from the module builder's nix store)
S03	PASS	lgx version (v0.2-era, from the module builder's nix store)
S04	PASS	lgpm version (v0.2-era, from the module builder's nix store)
S05	PASS	logoscore version (the v0.2-era runtime in our images)
S06	PASS	start a clean session daemon
S07	PASS	package show lez_atomic_swap_maker
S08	FAIL	lez_atomic_swap_maker is signed by a signer that can be pinned
S09	PASS	package show lez_core
S10	FAIL	lez_core is signed by a signer that can be pinned
S11	PASS	package show lez_swap_node
S12	FAIL	lez_swap_node is signed by a signer that can be pinned
S13	PASS	package show lez_maker_node
S14	FAIL	lez_maker_node is signed by a signer that can be pinned
S15	PASS	package show chat_module
S16	FAIL	chat_module is signed by a signer that can be pinned
S17	PASS	package show delivery_module
S18	FAIL	delivery_module is signed by a signer that can be pinned
S19	PASS	package show lez_indexer_module
S20	FAIL	lez_indexer_module is signed by a signer that can be pinned
S21	PASS	package show blockchain_module
S22	FAIL	blockchain_module is signed by a signer that can be pinned
S23	PASS	lgx verify lez_atomic_swap_maker (structure, Merkle root)
S24	PASS	lgx verify lez_core (structure, Merkle root)
S25	PASS	lgx verify lez_swap_node (structure, Merkle root)
S26	PASS	lgx verify lez_maker_node (structure, Merkle root)
S27	PASS	lgx verify chat_module (structure, Merkle root)
S28	PASS	lgx verify delivery_module (structure, Merkle root)
S29	PASS	lgx verify lez_indexer_module (structure, Merkle root)
S30	PASS	lgx verify blockchain_module (structure, Merkle root)
S31	PASS	lm metadata lez_atomic_swap_maker
S32	PASS	lm methods lez_atomic_swap_maker
S33	PASS	lm events lez_atomic_swap_maker
S34	PASS	lm metadata lez_core
S35	PASS	lm methods lez_core
S36	PASS	lm events lez_core
S37	PASS	lm metadata lez_swap_node
S38	PASS	lm methods lez_swap_node
S39	PASS	lm events lez_swap_node
S40	PASS	lm metadata lez_maker_node
S41	PASS	lm methods lez_maker_node
S42	PASS	lm events lez_maker_node
S43	PASS	lm metadata chat_module
S44	PASS	lm methods chat_module
S45	PASS	lm events chat_module
S46	PASS	lm metadata delivery_module
S47	PASS	lm methods delivery_module
S48	PASS	lm events delivery_module
S49	PASS	lm metadata lez_indexer_module
S50	PASS	lm methods lez_indexer_module
S51	PASS	lm events lez_indexer_module
S52	PASS	lm metadata blockchain_module
S53	PASS	lm methods blockchain_module
S54	PASS	lm events blockchain_module
S55	NOT RUN	lm verify <installed dir> --did <pinned signer>
S56	FAIL	install lez_atomic_swap_maker as built
S57	INFO	DERIVED: lez_atomic_swap_maker with variant linux-amd64-dev renamed linux-amd64 (lgx extract/add/remove)
S58	PASS	package show (derived)
S59	PASS	install lez_atomic_swap_maker (derived) with its root hash
S60	FAIL	installing the role package pulled in its dependencies
S61	FAIL	the role package declares its LEZ network requirement
S62	PASS	install lez_core with its root hash
S63	PASS	install lez_swap_node with its root hash
S64	PASS	install lez_maker_node with its root hash
S65	FAIL	install chat_module with its root hash
S66	FAIL	install delivery_module with its root hash
S67	PASS	install lez_indexer_module with its root hash
S68	PASS	install blockchain_module with its root hash
S69	FAIL	a wrong --root-hash is refused
S70	FAIL	every dependency of lez_atomic_swap_maker is installed
S71	PASS	package ls
S72	FAIL	module load lez_atomic_swap_maker
S73	PASS	module load lez_swap_node (with its declared dependencies)
S74	PASS	module show lez_swap_node
S75	PASS	call lez_swap_node node_status maker
S76	PASS	the installed package graph carries the maker Node and its launcher
S77	PASS	module load lez_maker_node (the maker Node package, with its declared dependencies)
S78	PASS	call lez_maker_node launcher: the installed launcher
S79	PASS	call lez_maker_node payload: the installed files and their sha256
S80	PASS	start_node with the package's launcher
S81	PASS	lez_swap_node launched the maker Node from the package
S82	INFO	the maker Node's output (without the role's configuration, which Compose provides, it stops at once)
S83	INFO	STUB: a stand-in launcher (sleeps; not a Node)
S84	PASS	STUB: start_node with the stand-in launcher
S85	PASS	STUB: node_status reports the supervised child
S86	PASS	module stats
S87	FAIL	module stats shows the maker Node (pid 358) itself
S88	PASS	module reload lez_swap_node
S89	PASS	the maker Node is supervised again after the reload
S90	INFO	processes after the reload
S91	INFO	kill -9 the lez_swap_node host process
S92	FAIL	the runtime reports lez_swap_node as crashed
S93	PASS	no orphaned maker Node after the host crash
S94	INFO	load lez_swap_node again and start the role again (what a desk does)
S95	PASS	exactly one maker Node after recovery
S96	PASS	package remove lez_atomic_swap_maker
S97	PASS	package remove lez_core (takes its dependents)
S98	PASS	nothing of ours is left installed
S99	INFO	what remove leaves in the session
S100	PASS	daemon stop
S101	PASS	logosctl 0.3.0: install, load lez_swap_node, call node_status
S102	FAIL	logosctl 0.3.0: install lez_atomic_swap_maker as built
S103	PASS	logoscore 0.2.3: modules laid out as the images do (no installer in this release)
S104	PASS	logoscore 0.2.3: load-module lez_swap_node, call node_status
S105	FAIL	lgpm (v0.2-era, the desks' installer): install lez_swap_node
S106	PASS	lgpm (v0.2-era, the desks' installer): install lez_atomic_swap_maker
```

106 steps, 20 judged FAIL.
