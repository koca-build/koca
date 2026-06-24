//! Integration tests for parsing relationship fields (`depends`, `provides`,
//! `conflicts`) from `.koca` files.

use std::path::{Path, PathBuf};

use koca::rfpm::relation::Op;
use koca::BuildFile;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[tokio::test]
async fn parses_relationship_fields() {
    let bf = BuildFile::parse_file(fixture("relationships.koca"))
        .await
        .unwrap();

    assert_eq!(bf.depends().len(), 2);
    assert_eq!(bf.depends()[0].name, "libc6");
    assert!(bf.depends()[0].constraint.is_none());
    let dep = bf.depends()[1].constraint.as_ref().unwrap();
    assert_eq!((dep.op, dep.version.as_str()), (Op::GreaterEqual, "3.0"));

    assert_eq!(bf.provides().len(), 2);
    assert_eq!(bf.provides()[0].name, "bun");
    assert_eq!(bf.provides()[0].version, None);
    assert_eq!(bf.provides()[1].name, "cron");
    assert_eq!(bf.provides()[1].version.as_deref(), Some("2.0"));

    assert_eq!(bf.conflicts().len(), 1);
    let conf = bf.conflicts()[0].constraint.as_ref().unwrap();
    assert_eq!((conf.op, conf.version.as_str()), (Op::Less, "5"));
}

#[tokio::test]
async fn provides_rejects_range_operator() {
    assert!(BuildFile::parse_file(fixture("provides-range.koca"))
        .await
        .is_err());
}
