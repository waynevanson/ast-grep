use crate::check_var::{check_fixer_vars, check_rewriter_fix};
use crate::fixer::{Fixer, FixerError, SerializableFixer};
use crate::rewriter::{Rewriter, RewriterError, SerializableRewriter};
use crate::rule::DeserializeEnv;
use crate::rule_core::{RuleCore, RuleCoreError, SerializableRuleCore};

use ast_grep_core::language::Language;
use globset::{Glob, GlobSet, GlobSetBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use std::collections::HashSet;

/// A named, reusable cross-file rewrite definition.
///
/// A multi-writer is intentionally a traversal definition rather than a
/// larger `Fixer`. Rewriters still perform the actual per-document edits,
/// while the multi-writer describes which related files to visit and how
/// many times the traversal may recurse.
pub struct MultiWriter {
  pub id: String,
  pub follow: Vec<FollowRule>,
  pub search: Vec<SearchRule>,
  pub create: Vec<CreateRule>,
  pub rewrite: Vec<Rewriter>,
  pub recursive: bool,
  pub max_depth: usize,
  pub max_files: usize,
}

pub struct FollowRule {
  pub matcher: RuleCore,
  pub path: Fixer,
  pub resolve: FollowResolve,
}

pub struct SearchRule {
  pub matcher: RuleCore,
  pub files: GlobSet,
}

/// Creates a new file from a template after discovery completes.
///
/// The path is relative to the matched seed file. The template can use
/// variables gathered from the seed and every discovered file.
pub struct CreateRule {
  pub path: Fixer,
  pub template: Fixer,
  pub overwrite: bool,
}

const DEFAULT_MAX_DEPTH: usize = 8;
const DEFAULT_MAX_FILES: usize = 1_000;

#[derive(Debug, Error)]
#[error("Multi-writer `{id}` has invalid configuration.")]
pub struct MultiWriterError {
  pub id: String,
  #[source]
  pub reason: MultiWriterErrorReason,
}

#[derive(Debug, Error)]
pub enum MultiWriterErrorReason {
  #[error(transparent)]
  Core(#[from] RuleCoreError),
  #[error(transparent)]
  Rewriter(#[from] RewriterError),
  #[error(transparent)]
  Fixer(#[from] FixerError),
  #[error(transparent)]
  Glob(#[from] globset::Error),
}

#[derive(Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SerializableMultiWriter {
  /// Unique, descriptive identifier, e.g. rename-references
  pub id: String,
  /// Rules that discover files related to the current file.
  #[serde(default)]
  pub follow: Vec<SerializableFollow>,
  /// Rules that search a candidate file set for references.
  #[serde(default)]
  pub search: Vec<SerializableSearch>,
  /// New files to create from templates after discovery.
  #[serde(default)]
  pub create: Vec<SerializableCreate>,
  /// Rewriters applied to every visited file, including the seed file.
  pub rewrite: Vec<SerializableRewriter>,
  /// Whether to continue discovering related files from visited files.
  #[serde(default)]
  pub recursive: bool,
  /// Maximum traversal depth. Defaults to 8.
  pub max_depth: Option<usize>,
  /// Maximum number of files visited for one seed. Defaults to 1000.
  pub max_files: Option<usize>,
}

#[derive(Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SerializableFollow {
  #[serde(flatten)]
  pub core: SerializableRuleCore,
  /// Template that resolves to the related file path, e.g. `$SPEC`.
  pub path: String,
  /// How to interpret the rendered path.
  #[serde(default)]
  pub resolve: FollowResolve,
}

#[derive(Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SerializableSearch {
  #[serde(flatten)]
  pub core: SerializableRuleCore,
  /// Globs for candidate files, relative to the project root.
  #[serde(default)]
  pub files: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SerializableCreate {
  /// Path template, relative to the matched seed file.
  pub path: String,
  /// Initial file content template.
  pub template: String,
  /// Replace an existing file. Defaults to false.
  #[serde(default)]
  pub overwrite: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FollowResolve {
  /// Interpret the path relative to the current file.
  #[default]
  Relative,
  /// Interpret the path as a module specifier. Relative specifiers and
  /// extension/index probing are supported.
  Module,
}

impl SerializableMultiWriter {
  pub fn try_parse<L: Language>(
    &self,
    upper_vars: &HashSet<&str>,
    env: &DeserializeEnv<L>,
  ) -> Result<MultiWriter, MultiWriterError> {
    let attach_id = |reason| MultiWriterError {
      id: self.id.clone(),
      reason,
    };
    let mut follow = Vec::with_capacity(self.follow.len());
    for rule in &self.follow {
      let matcher = rule
        .core
        .get_matcher(env.clone())
        .map_err(|e| attach_id(e.into()))?;
      let fix = SerializableFixer::Str(rule.path.clone());
      let mut path =
        Fixer::parse(&fix, env, &rule.core.transform).map_err(|e| attach_id(e.into()))?;
      let path = path.pop().expect("fixer string always has one fixer");
      check_rewriter_fix(&matcher, std::slice::from_ref(&path), upper_vars)
        .map_err(|e| attach_id(e.into()))?;
      follow.push(FollowRule {
        matcher,
        path,
        resolve: rule.resolve,
      });
    }

    let mut search = Vec::with_capacity(self.search.len());
    for rule in &self.search {
      let matcher = rule
        .core
        .get_matcher(env.clone())
        .map_err(|e| attach_id(e.into()))?;
      let mut builder = GlobSetBuilder::new();
      for glob in &rule.files {
        builder.add(Glob::new(glob).map_err(|e| attach_id(e.into()))?);
      }
      search.push(SearchRule {
        matcher,
        files: builder.build().map_err(|e| attach_id(e.into()))?,
      });
    }

    let mut allowed_vars = upper_vars.clone();
    for rule in &follow {
      allowed_vars.extend(rule.matcher.defined_vars());
    }
    for rule in &search {
      allowed_vars.extend(rule.matcher.defined_vars());
    }
    let mut create = Vec::with_capacity(self.create.len());
    for rule in &self.create {
      let path = Fixer::from_str(&rule.path, &env.lang).map_err(|e| attach_id(e.into()))?;
      let template = Fixer::from_str(&rule.template, &env.lang).map_err(|e| attach_id(e.into()))?;
      check_fixer_vars(&allowed_vars, &path).map_err(|e| attach_id(e.into()))?;
      check_fixer_vars(&allowed_vars, &template).map_err(|e| attach_id(e.into()))?;
      create.push(CreateRule {
        path,
        template,
        overwrite: rule.overwrite,
      });
    }

    let mut rewrite = Vec::with_capacity(self.rewrite.len());
    for rewriter in &self.rewrite {
      let parsed = rewriter
        .try_parse_rewriter(&allowed_vars, env)
        .map_err(|e| attach_id(e.into()))?;
      rewrite.push(parsed);
    }

    Ok(MultiWriter {
      id: self.id.clone(),
      follow,
      search,
      create,
      rewrite,
      recursive: self.recursive,
      max_depth: self.max_depth.unwrap_or(DEFAULT_MAX_DEPTH).max(1),
      max_files: self.max_files.unwrap_or(DEFAULT_MAX_FILES).max(1),
    })
  }
}
