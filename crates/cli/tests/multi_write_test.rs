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

fn simple_rename_rule() -> &'static str {
  r#"
id: simple-rename
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
  - writer
multiWriters:
  - id: writer
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
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
    .success()
    .stderr(predicate::str::contains("multi-writer variable conflict"));

  assert_eq!(
    std::fs::read_to_string(dir.path().join("generated-newName.ts"))?,
    "'./a'\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_create_overwrite() -> Result<()> {
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
        template: "overwritten $NEW\n"
        overwrite: true
    rewrite: []
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
    ("generated-newName.ts", "keep me\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("generated-newName.ts"))?,
    "overwritten newName\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_create_nested_directories() -> Result<()> {
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
      - path: './nested/deep/generated-$NEW.ts'
        template: "nested $NEW\n"
    rewrite: []
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("nested/deep/generated-newName.ts"))?,
    "nested newName\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_create_rejects_root_escape() -> Result<()> {
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
      - path: '../multi-writer-root-escape.ts'
        template: "escaped\n"
    rewrite: []
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success()
    .stderr(predicate::str::contains("cannot resolve created file path"));

  let escaped = dir
    .path()
    .parent()
    .unwrap()
    .join("multi-writer-root-escape.ts");
  assert!(!escaped.exists());
  Ok(())
}

#[test]
fn test_multi_writer_rule_flag() -> Result<()> {
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: []\n"),
    ("rules/rename.yml", simple_rename_rule()),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--rule", "rules/rename.yml", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("a.ts"))?,
    "export const newName = 1;\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_inline_rules_flag() -> Result<()> {
  let dir = create_test_files([("a.ts", "export const oldName = 1;\n")])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args([
      "multi-write",
      "--inline-rules",
      simple_rename_rule(),
      "--update-all",
    ])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("a.ts"))?,
    "export const newName = 1;\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_max_depth() -> Result<()> {
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
    maxDepth: 1
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
    "const oldName = 2;\nexport { oldName };\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_max_files() -> Result<()> {
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
    maxFiles: 1
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
    std::fs::read_to_string(dir.path().join("b.ts"))?,
    "import { oldName } from './c';\nexport { oldName };\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_ignores_bare_module_specifier() -> Result<()> {
  let rule = r#"
id: rename-references
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
  - rename-references
multiWriters:
  - id: rename-references
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: module
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
      "export const oldName = 1;\nimport { oldName } from 'pkg';\n",
    ),
    ("pkg.ts", "const oldName = 2;\nexport { oldName };\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("pkg.ts"))?,
    "const oldName = 2;\nexport { oldName };\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_ignores_cross_language_follow() -> Result<()> {
  let rule = r#"
id: rename-references
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
  - rename-references
multiWriters:
  - id: rename-references
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: module
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
      "export const oldName = 1;\nimport { oldName } from './other.rs';\n",
    ),
    ("other.rs", "pub const oldName: i32 = 2;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("other.rs"))?,
    "pub const oldName: i32 = 2;\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_create_skips_existing_by_default() -> Result<()> {
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
        template: "replaced\n"
    rewrite: []
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
    ("generated-newName.ts", "keep me\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("generated-newName.ts"))?,
    "keep me\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_follow_relative_resolution() -> Result<()> {
  let rule = r#"
id: rename-relative
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
  - rename-relative
multiWriters:
  - id: rename-relative
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: relative
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
    recursive: false
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/rename.yml", rule),
    (
      "a.ts",
      "export const oldName = 1;\nimport { oldName } from './b';\n",
    ),
    ("b.ts", "const oldName = 2;\nexport { oldName };\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("b.ts"))?,
    "const newName = 2;\nexport { newName };\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_no_multi_fix_rules() -> Result<()> {
  let rule = r#"
id: plain-rule
language: TypeScript
rule:
  pattern: 'export const $OLD = $INIT'
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/plain.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success()
    .stderr(predicate::str::contains("No multi-writer seeds found"));

  assert_eq!(
    std::fs::read_to_string(dir.path().join("a.ts"))?,
    "export const oldName = 1;\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_resolves_js_specifier_to_ts_file() -> Result<()> {
  let rule = r#"
id: rename-references
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
  - rename-references
multiWriters:
  - id: rename-references
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: module
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
    recursive: false
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/rename.yml", rule),
    (
      "a.ts",
      "export const oldName = 1;\nimport { oldName } from './b.js';\n",
    ),
    ("b.ts", "const oldName = 2;\nexport { oldName };\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(
    std::fs::read_to_string(dir.path().join("b.ts"))?,
    "const newName = 2;\nexport { newName };\n"
  );
  Ok(())
}

#[test]
fn test_multi_writer_create_absolute_path() -> Result<()> {
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;
  std::fs::create_dir_all(dir.path().join("rules"))?;
  let absolute = dir.path().canonicalize()?.join("absolute-newName.ts");
  let rule = format!(
    r#"
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
      - path: '{}'
        template: "absolute $NEW\n"
    rewrite: []
"#,
    absolute.display()
  );
  std::fs::write(dir.path().join("rules/generate.yml"), rule)?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  assert_eq!(std::fs::read_to_string(absolute)?, "absolute newName\n");
  Ok(())
}

#[test]
fn test_multi_writer_create_empty_path_is_rejected() -> Result<()> {
  let rule = r#"
id: generate-file
language: TypeScript
rule:
  pattern: 'export const $OLD = $INIT'
multiFix:
  - generate-file
multiWriters:
  - id: generate-file
    create:
      - path: ''
        template: "ignored\n"
    rewrite: []
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/generate.yml", rule),
    ("a.ts", "export const oldName = 1;\n"),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success()
    .stderr(predicate::str::contains("cannot resolve created file path"));
  Ok(())
}

#[test]
fn test_multi_writer_multiple_seeds_share_overlay() -> Result<()> {
  let rename = r#"
id: rename-references
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
  - rename-references
multiWriters:
  - id: rename-references
    search:
      - files: ['**/*.ts']
        rule:
          pattern: 'import { $OLD } from $SPEC'
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
    recursive: false
"#;
  let plain = r#"
id: plain-rule
language: TypeScript
rule:
  pattern: 'console.log($A)'
"#;
  let dir = create_test_files([
    ("sgconfig.yml", "ruleDirs: [rules]\n"),
    ("rules/rename.yml", rename),
    ("rules/plain.yml", plain),
    (
      "a1.ts",
      "export const oldName = 1;\nimport { oldName } from './a2';\n",
    ),
    (
      "a2.ts",
      "export const oldName = 2;\nimport { oldName } from './a1';\n",
    ),
  ])?;

  Command::new(cargo_bin!())
    .current_dir(dir.path())
    .args(["multi-write", "--update-all"])
    .assert()
    .success();

  for file in ["a1.ts", "a2.ts"] {
    let content = std::fs::read_to_string(dir.path().join(file))?;
    assert!(
      content.contains("newName"),
      "{file} was not rewritten: {content}"
    );
    assert!(
      !content.contains("oldName"),
      "{file} kept oldName: {content}"
    );
  }
  Ok(())
}
