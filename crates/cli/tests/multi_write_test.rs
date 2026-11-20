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

#[test]
fn test_multi_writer_creates_file_with_discovered_vars() -> Result<()> {
  let rule = r#"
id: generate-file
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
  - generate-file
multiWriters:
  - id: generate-file
    search:
      - files: ['b.ts']
        rule:
          pattern: 'import { $OLD } from $SPEC'
    create:
      - path: 'generated-$NEW.ts'
        template: "import { $NEW } from $SPEC;\n"
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
    ("b.ts", "import { oldName } from './a';\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("generated-newName.ts"))?,
    "import { newName } from './a';\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_create_dry_run_does_not_write() -> Result<()> {
  let rule = r#"
id: generate-file
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
  - generate-file
multiWriters:
  - id: generate-file
    create:
      - path: 'generated-$NEW.ts'
        template: "export const $NEW = $INIT;\n"
    rewrite: []
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write"])
    .assert()
    .success()
    .stdout(predicate::str::contains("generated-newName.ts"))
    .stdout(predicate::str::contains("export const newName = 1;"));

  assert!(!dir.path().join("generated-newName.ts").exists());
  Ok(())
}

#[test]
fn test_multi_writer_discovery_vars_do_not_override() -> Result<()> {
  let rule = r#"
id: generate-file
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
  - generate-file
multiWriters:
  - id: generate-file
    search:
      - files: ['b.ts', 'c.ts']
        rule:
          pattern: 'import { $OLD } from $SPEC'
    create:
      - path: 'generated-$NEW.ts'
        template: "$SPEC\n"
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
    ("b.ts", "import { oldName } from './a';\n"),
    ("c.ts", "import { oldName } from './b';\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("generated-newName.ts"))?,
    "'./a'\n"
  );
  Ok(())
}
