//! regenesis <deployment-settings.yaml> <pk> <value>: rebuild the genesis block
//! with every transfer output owned by <pk> set to <value>; prints the new
//! block_root and the SDP declaration's locked_note_id (both change with the
//! transfer op), so the YAML can be patched in place.
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
    let doc: Value = serde_yml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let ops = &doc["cryptarchia"]["genesis_block"]["transactions"][0]["mantle_tx"]["ops"];
    let mut outputs = ops[0]["payload"]["outputs"].clone();
    let old_notes: Vec<Note> = serde_yml::from_value(outputs.clone()).unwrap();
    for out in outputs.as_sequence_mut().unwrap() {
        if out["pk"].as_str() == Some(pk) {
            out["value"] = Value::Number(value.into());
        }
    }
    let notes: Vec<Note> = serde_yml::from_value(outputs).unwrap();
    let inscription: InscriptionOp = serde_yml::from_value(ops[1]["payload"].clone()).unwrap();
    let mut decl: SDPDeclareOp = serde_yml::from_value(ops[2]["payload"].clone()).unwrap();

    let old_op = TransferOp::new(Inputs::empty(), Outputs::try_new(old_notes).unwrap());
    let new_op = TransferOp::new(Inputs::empty(), Outputs::try_new(notes.clone()).unwrap());
    let index = (0..notes.len())
        .find(|i| old_op.utxo_by_index(*i).unwrap().id() == decl.locked_note_id)
        .expect("locked note among the genesis outputs");
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
}
