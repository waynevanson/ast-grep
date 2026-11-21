use std::fs::{create_dir_all, read_to_string, write};
use std::path::Path;

use anyhow::Result;
use assert_cmd::{Command, cargo_bin};
use predicates::str::contains;
use tempfile::TempDir;

/// A barrel written the way a user writes one: a rule matches the whole file and
/// its `transform.files` reads the directory the file sits in.
const RULE: &str = r#"id: barrel-exports
language: ts
severity: error
files:
  - "**/index.ts"
rule:
  kind: program
transform:
  EXPORTS:
    files:
      globs: ["*.ts", "*.tsx"]
      ignores: ["*.test.ts", "index.ts", "internal.ts"]
      template: |
        export * from "./{{stem}}";
fix: $EXPORTS
"#;

const BARREL: &str = "export * from \"./A\";\nexport * from \"./B\";\n";

fn sg(dir: &Path) -> Command {
  let mut cmd = Command::new(cargo_bin!("ast-grep"));
  cmd.current_dir(dir);
  cmd
}

fn tree(files: impl IntoIterator<Item = (&'static str, &'static str)>) -> Result<TempDir> {
  let dir = TempDir::new()?;
  let mut all = vec![
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/barrel.yml", RULE),
    ("src/components/A.ts", "export const A = 1;\n"),
    ("src/components/B.ts", "export const B = 1;\n"),
    ("src/components/A.test.ts", "export const test = 1;\n"),
    ("src/components/internal.ts", "export const internal = 1;\n"),
  ];
  all.extend(files);
  for (name, contents) in all {
    let path = dir.path().join(name);
    if let Some(parent) = path.parent() {
      create_dir_all(parent)?;
    }
    write(path, contents)?;
  }
  Ok(dir)
}

fn read(dir: &Path, name: &str) -> Result<String> {
  Ok(read_to_string(dir.join(name))?)
}

#[test]
fn scan_writes_a_barrel_from_the_directory_the_rule_matched() -> Result<()> {
  let dir = tree([("src/components/index.ts", "")])?;
  sg(dir.path()).args(["scan", "-U"]).assert().success();
  assert_eq!(
    read(dir.path(), "src/components/index.ts")?,
    BARREL,
    "one sorted line per module, tests, internals and the barrel itself left out"
  );
  Ok(())
}

#[test]
fn a_barrel_that_already_says_it_is_written_as_it_is() -> Result<()> {
  let dir = tree([("src/components/index.ts", BARREL)])?;
  sg(dir.path()).args(["scan", "-U"]).assert().success();
  assert_eq!(
    read(dir.path(), "src/components/index.ts")?,
    BARREL,
    "the rule matches the whole file, so it writes a file that already says it"
  );
  Ok(())
}

#[test]
fn a_stale_file_fails_the_scan_without_being_written() -> Result<()> {
  let dir = tree([("src/components/index.ts", "export * from \"./A\";\n")])?;
  sg(dir.path())
    .arg("scan")
    .assert()
    .failure()
    .stdout(contains("barrel-exports"));
  assert_eq!(
    read(dir.path(), "src/components/index.ts")?,
    "export * from \"./A\";\n"
  );
  Ok(())
}

#[test]
fn every_directory_gets_its_own_barrel() -> Result<()> {
  let dir = tree([
    ("src/components/index.ts", ""),
    ("src/components/Sub/C.tsx", "export const C = 1;\n"),
    ("src/components/Sub/index.ts", ""),
  ])?;
  sg(dir.path()).args(["scan", "-U"]).assert().success();
  assert_eq!(read(dir.path(), "src/components/index.ts")?, BARREL);
  assert_eq!(
    read(dir.path(), "src/components/Sub/index.ts")?,
    "export * from \"./C\";\n"
  );
  Ok(())
}

#[test]
fn a_file_the_rule_does_not_match_is_left_alone() -> Result<()> {
  let dir = tree([("src/components/other.ts", "export const other = 1;\n")])?;
  sg(dir.path()).args(["scan", "-U"]).assert().success();
  assert_eq!(
    read(dir.path(), "src/components/other.ts")?,
    "export const other = 1;\n"
  );
  Ok(())
}
