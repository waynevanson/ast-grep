use std::fs::read_dir;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Renders one string per file matching `globs` in the directory of the file
/// being scanned, in file name order.
///
/// A rule matches a node, and a directory is not a node, so this is how a rule
/// can say something about the files beside the one it matched: match the whole
/// file, render what its siblings are, and write that. A barrel — one re-export
/// per module next to `index.ts` — is one use of it.
#[derive(Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Files {
  /// Glob patterns, matched against a file name, of the files to render.
  pub globs: Vec<String>,
  /// Glob patterns, matched against a file name, of the files to skip.
  pub ignores: Option<Vec<String>>,
  /// What to render for a file: `{{name}}` is its file name, `{{stem}}` its file
  /// name without the extension, `{{ext}}` the extension with its dot.
  pub template: String,
  /// Joins the rendered strings, nothing by default, so a template written as a
  /// YAML block scalar can carry its own line break.
  pub join_by: Option<String>,
}

impl Files {
  /// The `globs` and `ignores`, compiled. Both are matched against a file name,
  /// not a path, because the directory is the one being rendered.
  pub fn compile(&self) -> Result<(GlobSet, Option<GlobSet>), globset::Error> {
    let ignores = self.ignores.as_ref().map(|ignores| compile(ignores));
    Ok((compile(&self.globs)?, ignores.transpose()?))
  }

  pub(crate) fn compute(&self) -> Option<String> {
    let file = crate::path_context::current_file()?;
    let dir = file.parent()?;
    let (globs, ignores) = self.compile().ok()?;
    let mut names = vec![];
    for entry in read_dir(dir).ok()? {
      let entry = entry.ok()?;
      if !entry.file_type().ok()?.is_file() {
        continue;
      }
      let name = entry.file_name();
      let Some(name) = name.to_str() else {
        continue;
      };
      let ignored = ignores.as_ref().is_some_and(|set| set.is_match(name));
      if !globs.is_match(name) || ignored {
        continue;
      }
      names.push(name.to_owned());
    }
    names.sort();
    let rendered: Vec<String> = names.iter().map(|name| self.render(name)).collect();
    Some(rendered.join(self.join_by.as_deref().unwrap_or("")))
  }

  fn render(&self, name: &str) -> String {
    let path = Path::new(name);
    let stem = path
      .file_stem()
      .and_then(|stem| stem.to_str())
      .unwrap_or(name);
    let ext = match path.extension().and_then(|ext| ext.to_str()) {
      Some(ext) if !ext.is_empty() => format!(".{ext}"),
      _ => String::new(),
    };
    self
      .template
      .replace("{{name}}", name)
      .replace("{{stem}}", stem)
      .replace("{{ext}}", &ext)
  }
}

fn compile(globs: &[String]) -> Result<GlobSet, globset::Error> {
  let mut builder = GlobSetBuilder::new();
  for glob in globs {
    builder.add(Glob::new(glob)?);
  }
  builder.build()
}

#[cfg(test)]
mod test {
  use super::*;
  use crate::path_context::with_file;
  use std::fs::{create_dir_all, write};
  use tempfile::TempDir;

  fn tree(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    for (name, contents) in files {
      let path = dir.path().join(name);
      if let Some(parent) = path.parent() {
        create_dir_all(parent).expect("directory");
      }
      write(path, contents).expect("file");
    }
    dir
  }

  fn files(yaml: &str) -> Files {
    crate::from_str(yaml).expect("should parse")
  }

  #[test]
  fn renders_one_string_per_matching_file_in_name_order() {
    let dir = tree(&[
      ("components/B.ts", ""),
      ("components/A.tsx", ""),
      ("components/A.test.ts", ""),
      ("components/index.ts", ""),
      ("elsewhere/C.ts", ""),
    ]);
    let spec = files(
      r#"
      globs: ["*.ts", "*.tsx"]
      ignores: ["*.test.ts", "index.ts"]
      template: 'export * from "./{{stem}}";\n'
    "#,
    );
    let path = dir.path().join("components/index.ts");
    let rendered = with_file(&path, || spec.compute()).expect("should render");
    assert_eq!(
      rendered,
      "export * from \"./A\";\\nexport * from \"./B\";\\n"
    );
  }

  #[test]
  fn joins_with_join_by_and_fills_the_name_and_extensions() {
    let dir = tree(&[("a.ts", ""), ("b.tsx", "")]);
    let spec = files(
      r#"
      globs: ["*.ts*"]
      template: "{{name}}|{{stem}}|{{ext}}"
      joinBy: "\n"
    "#,
    );
    let path = dir.path().join("c.ts");
    let rendered = with_file(&path, || spec.compute()).expect("should render");
    assert_eq!(rendered, "a.ts|a|.ts\nb.tsx|b|.tsx");
  }

  #[test]
  fn a_file_scanned_outside_a_path_has_no_siblings() {
    let spec = files(
      r#"
      globs: ["*.ts"]
      template: "{{name}}"
    "#,
    );
    assert_eq!(spec.compute(), None);
  }
}
