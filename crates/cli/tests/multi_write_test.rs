mod common;

use anyhow::Result;
use assert_cmd::{Command, cargo_bin};
use common::create_test_files;
use predicates::prelude::*;

fn rename_rule() -> &'static str {
  r#"
id: rename-export
language: TypeScript
rule:
  pattern: 'export const $OLD = $INIT'
transform:
  NEW:
    replace:
      source: $OLD
      replace: old
      by: new
fix: 'export const $NEW = $INIT;'
multiFix:
  - rename-references
multiWriters:
  - id: rename-references
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: module
    search:
      - files: ['**/*.ts']
        rule:
          pattern: 'import { $OLD } from $SPEC'
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
    recursive: true
    maxDepth: 8
    maxFiles: 100
"#
}

#[test]
fn test_multi_writer_recursive_rename() -> Result<()> {
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/rename.yml", rename_rule()),
    ("a.ts", "export const oldName = 1;\n"),
    (
      "b.ts",
      "import { oldName } from './a';\nconsole.log(oldName);\n",
    ),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  let a = std::fs::read_to_string(dir.path().join("a.ts"))?;
  let b = std::fs::read_to_string(dir.path().join("b.ts"))?;
  assert_eq!(a, "export const newName = 1;\n");
  assert_eq!(b, "import { newName } from './a';\nconsole.log(newName);\n");
  Ok(())
}

#[test]
fn test_multi_writer_dry_run() -> Result<()> {
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/rename.yml", rename_rule()),
    ("a.ts", "export const oldName = 1;\n"),
    ("b.ts", "import { oldName } from './a';\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write"])
    .assert()
    .success()
    .stdout(predicate::str::contains("a.ts"))
    .stdout(predicate::str::contains("b.ts"))
    .stdout(predicate::str::contains("newName"));

  assert_eq!(
    std::fs::read_to_string(dir.path().join("a.ts"))?,
    "export const oldName = 1;\n"
  );
  assert_eq!(
    std::fs::read_to_string(dir.path().join("b.ts"))?,
    "import { oldName } from './a';\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_follow_recursion() -> Result<()> {
  let rule = r#"
id: rename-chain
language: TypeScript
rule:
  pattern: 'export const $OLD = $INIT'
transform:
  NEW:
    replace:
      source: $OLD
      replace: old
      by: new
multiFix:
  - rename-chain
multiWriters:
  - id: rename-chain
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: module
    search:
      - files: ['b.ts']
        rule:
          pattern: 'import { $OLD } from $SPEC'
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
    recursive: true
    maxDepth: 8
    maxFiles: 100
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/rename.yml", rule),
    (
      "a.ts",
      "export const oldName = 1;\nimport { oldName } from './b';\n",
    ),
    (
      "b.ts",
      "import { oldName } from './c';\nexport { oldName };\n",
    ),
    ("c.ts", "const oldName = 2;\nexport { oldName };\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("c.ts"))?,
    "const newName = 2;\nexport { newName };\n"
  );
  Ok(())
}
