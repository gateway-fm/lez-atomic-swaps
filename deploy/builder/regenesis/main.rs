//! regenesis <deployment-settings.yaml> <pk> <value> [<n> <pk2>] [time=<unix>]:
//! rebuild the genesis block with every transfer output owned by <pk> set to
//! <value>, the last <n> of them (never the SDP declaration's locked note) owned
//! by <pk2> instead and, given time=<unix>, the chain's genesis time (in the
//! genesis inscription's Cryptarchia parameters) set to it. Prints the new
//! block_root, the SDP declaration's locked_note_id (it changes with the
//! transfer op), the index of that note among the outputs and the genesis
//! inscription, so the YAML can be patched in place.
use lb_core::{
    block::genesis::GenesisBlockBuilder,
    mantle::{
        Note,
        ledger::{Inputs, Outputs},
        ops::{channel::inscribe::InscriptionOp, sdp::SDPDeclareOp, transfer::TransferOp},
    },
};
use serde_yml::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, pk, value): (&str, &str, u64) = (&args[1], &args[2], args[3].parse().unwrap());
    let time: Option<u32> = args.iter().find_map(|a| a.strip_prefix("time=")).map(|t| t.parse().unwrap());
    let rest: Vec<&String> = args[4..].iter().filter(|a| !a.starts_with("time=")).collect();
    let moved: Option<(usize, &str)> = match rest.as_slice() {
        [n, pk2] => Some((n.parse().unwrap(), pk2.as_str())),
        [] => None,
        _ => panic!("expected <n> <pk2> after <value>"),
    };
    let doc: Value = serde_yml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let ops = &doc["cryptarchia"]["genesis_block"]["transactions"][0]["mantle_tx"]["ops"];

    let mut inscription_value = ops[1]["payload"].clone();
    if let Some(time) = time {
        // CryptarchiaParameter: chain_id (one length byte, then the bytes), the
        // genesis time (u32, little endian), the epoch nonce.
        let hex = inscription_value["inscription"].as_str().unwrap().to_owned();
        let at = 2 * (1 + usize::from_str_radix(&hex[..2], 16).unwrap());
        let le: String = time.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect();
        inscription_value["inscription"] = Value::String(format!("{}{le}{}", &hex[..at], &hex[at + 8..]));
    }
    let inscription: InscriptionOp = serde_yml::from_value(inscription_value.clone()).unwrap();
    let mut decl: SDPDeclareOp = serde_yml::from_value(ops[2]["payload"].clone()).unwrap();

    let mut outputs = ops[0]["payload"]["outputs"].clone();
    let old_notes: Vec<Note> = serde_yml::from_value(outputs.clone()).unwrap();
    let old_op = TransferOp::new(Inputs::empty(), Outputs::try_new(old_notes).unwrap());
    let index = (0..outputs.as_sequence().unwrap().len())
        .find(|i| old_op.utxo_by_index(*i).unwrap().id() == decl.locked_note_id)
        .expect("locked note among the genesis outputs");
    let owned: Vec<usize> = outputs
        .as_sequence()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, out)| out["pk"].as_str() == Some(pk))
        .map(|(i, _)| i)
        .collect();
    let seq = outputs.as_sequence_mut().unwrap();
    for i in &owned {
        seq[*i]["value"] = Value::Number(value.into());
    }
    if let Some((n, pk2)) = moved {
        let movable: Vec<usize> = owned.iter().copied().filter(|i| *i != index).collect();
        for i in &movable[movable.len() - n..] {
            seq[*i]["pk"] = Value::String(pk2.to_owned());
        }
    }
    let notes: Vec<Note> = serde_yml::from_value(outputs).unwrap();
    let new_op = TransferOp::new(Inputs::empty(), Outputs::try_new(notes.clone()).unwrap());
    decl.locked_note_id = new_op.utxo_by_index(index).unwrap().id();

    let mut it = notes.into_iter();
    let mut builder = GenesisBlockBuilder::new().add_note(it.next().unwrap());
    for n in it {
        builder = builder.try_add_note(n).unwrap();
    }
    let block = builder
        .set_inscription(inscription)
        .add_declaration(decl.clone())
        .build()
        .unwrap();
    let v = serde_yml::to_value(&block).unwrap();
    println!("block_root {}", serde_yml::to_string(&v["header"]["block_root"]).unwrap().trim());
    println!("locked_note_id {}", serde_yml::to_string(&serde_yml::to_value(decl.locked_note_id).unwrap()).unwrap().trim());
    println!("old_locked_index {index}");
    println!("inscription {}", inscription_value["inscription"].as_str().unwrap());
}
