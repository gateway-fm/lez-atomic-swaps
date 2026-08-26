use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
};

use lez_swap_store::SqliteXmrWorkflowJournal;
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn copied_header_and_table_names_cannot_forge_xmr_workflow_schema() {
    let root = tempdir().expect("isolated forged schema root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-private forged schema root");
    let database = root.path().join("forged-workflow.sqlite3");
    drop(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&database)
            .expect("new private forged database"),
    );
    let connection = Connection::open(&database).expect("open forged database");
    connection
        .execute_batch(
            "
            PRAGMA application_id = 1280857938;
            PRAGMA user_version = 1;
            CREATE TABLE xmr_workflow_identity (singleton_id INTEGER);
            CREATE TABLE xmr_workflow_steps (step TEXT);
            ",
        )
        .expect("forge only the public header and object names");
    drop(connection);

    assert!(
        SqliteXmrWorkflowJournal::open_existing(&database).is_err(),
        "copied headers and object names must not substitute for the exact strict schema"
    );
}
