//! Execute multi-writers by traversing files related to a rule match.
//!
//! The traversal is deliberately single-threaded per seed: a shared overlay keeps
//! recursive edits observable and deterministic, while the initial file walk still
//! uses the regular parallel walker.

use crate::config::{ProjectConfig, read_rule_file};
use crate::lang::SgLang;
use crate::print::ColorArg;
use crate::utils::{DiffStyles, InputArgs, RuleOverwrite, write_atomically, write_new_file};

use anyhow::Result;
use ast_grep_config::{
  CreateRule, Fixer, FollowResolve, FollowRule, GlobalRules, MultiWriter, RuleCollection,
  RuleConfig, from_yaml_string,
};
use ast_grep_core::meta_var::MetaVarEnv;
use ast_grep_core::replacer::Replacer;
use ast_grep_core::source::Edit;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_core::{Language, Matcher, NodeMatch};
use clap::Args;
use ignore::WalkState;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

#[derive(Args)]
pub struct MultiWriteArg {
  /// Run multi-writers from the rule located at RULE_FILE.
  #[clap(short, long, value_name = "RULE_FILE")]
  rule: Option<PathBuf>,

  /// Run multi-writers from rules defined by RULE_TEXT.
  ///
  /// Separate multiple rules with `---`.
  #[clap(long, conflicts_with = "rule", value_name = "RULE_TEXT")]
  inline_rules: Option<String>,

  /// Input related options.
  #[clap(flatten)]
  input: InputArgs,

  /// Apply all changes to disk.
  ///
  /// Without this flag, multi-write prints the pending diffs and leaves
  /// the files untouched.
  #[clap(short = 'U', long)]
  update_all: bool,

  /// Controls output color.
  #[clap(long, default_value = "auto", value_name = "WHEN")]
  color: ColorArg,
}

pub fn run_multi_write(mut arg: MultiWriteArg, project: Result<ProjectConfig>) -> Result<ExitCode> {
  if let Ok(config) = &project {
    arg.input.apply_config(config);
  }
  let (rules, root) = load_rules(&arg, project)?;
  let (seeds, candidates) = collect_seeds(&rules, &arg.input, &root)?;
  if seeds.is_empty() {
    eprintln!("No multi-writer seeds found.");
    return Ok(ExitCode::SUCCESS);
  }

  let mut session = MultiWriteSession::new(root, candidates);
  for seed in &seeds {
    run_seed(&mut session, &rules, seed)?;
  }
  let changed = session.finish(arg.update_all, arg.color)?;
  if arg.update_all && changed > 0 {
    eprintln!("Applied multi-writer changes to {changed} file(s).");
  }
  Ok(ExitCode::SUCCESS)
}

fn load_rules(
  arg: &MultiWriteArg,
  project: Result<ProjectConfig>,
) -> Result<(RuleCollection<SgLang>, PathBuf)> {
  let cwd = std::env::current_dir()?.canonicalize()?;
  if let Some(path) = &arg.rule {
    let globals = match &project {
      Ok(project) => project.find_utils()?,
      Err(_) => GlobalRules::default(),
    };
    let configs = read_rule_file(path, &globals)?;
    let (collection, _) = crate::config::with_rule_stats(configs)?;
    Ok((collection, cwd))
  } else if let Some(text) = &arg.inline_rules {
    let globals = match &project {
      Ok(project) => project.find_utils()?,
      Err(_) => GlobalRules::default(),
    };
    let configs = from_yaml_string(text, &globals)?;
    let (collection, _) = crate::config::with_rule_stats(configs)?;
    Ok((collection, cwd))
  } else {
    let project = project?;
    let root = project.project_dir.canonicalize()?;
    let (collection, _) = project.find_rules(RuleOverwrite::default())?;
    Ok((collection, root))
  }
}

#[derive(Clone)]
struct Seed {
  path: PathBuf,
  rule_id: String,
  writer_id: String,
  vars: HashMap<String, String>,
  start: usize,
  text: String,
}

fn read_source_file(path: &Path) -> Option<(PathBuf, SgLang, String)> {
  let path = path.canonicalize().ok()?;
  let lang = SgLang::from_path(&path)?;
  let content = std::fs::read_to_string(&path).ok()?;
  Some((path, lang, content))
}

fn collect_seeds(
  rules: &RuleCollection<SgLang>,
  input: &InputArgs,
  root: &Path,
) -> Result<(Vec<Seed>, Vec<PathBuf>)> {
  let mut langs = HashSet::new();
  rules.for_each_rule(|rule| {
    if !rule.multi_fix.is_empty() {
      langs.insert(rule.language);
    }
  });
  if langs.is_empty() {
    return Ok((vec![], vec![]));
  }

  let walker = input.walk_langs(langs.into_iter())?;
  let paths = Arc::new(Mutex::new(Vec::new()));
  let collected = paths.clone();
  walker.run(|| {
    let collected = collected.clone();
    Box::new(move |result| {
      if let Ok(entry) = result
        && entry
          .file_type()
          .is_some_and(|file_type| file_type.is_file())
      {
        collected.lock().unwrap().push(entry.into_path());
      }
      WalkState::Continue
    })
  });
  let mut paths = paths.lock().unwrap().clone();
  paths.sort();
  let mut candidates = vec![];
  let mut seeds = vec![];
  for (path, lang, content) in paths.into_iter().filter_map(|path| read_source_file(&path)) {
    candidates.push(path.clone());
    let grep = lang.ast_grep(content);
    let normalized = path.strip_prefix(root).unwrap_or(&path);
    for rule in rules.for_path(normalized) {
      if rule.multi_fix.is_empty() {
        continue;
      }
      for matched in grep.root().find_all(&rule.matcher) {
        let vars: HashMap<String, String> = matched.get_env().clone().into();
        let start = matched.range().start;
        let text = matched.text().to_string();
        for writer_id in &rule.multi_fix {
          seeds.push(Seed {
            path: path.clone(),
            rule_id: rule.id.clone(),
            writer_id: writer_id.clone(),
            vars: vars.clone(),
            start,
            text: text.clone(),
          });
        }
      }
    }
  }
  seeds.sort_by(|a, b| {
    (&a.path, &a.rule_id, &a.writer_id, a.start).cmp(&(&b.path, &b.rule_id, &b.writer_id, b.start))
  });
  Ok((seeds, candidates))
}

struct MultiWriteSession {
  root: PathBuf,
  /// Files discovered by the initial walk, used as search candidates.
  candidates: Vec<PathBuf>,
  /// Final content of every visited file, including files that were not edited.
  files: HashMap<PathBuf, String>,
  /// Content as it was on disk when the file was first visited.
  originals: HashMap<PathBuf, String>,
  /// Files that do not exist on disk and must be created on commit.
  created: HashSet<PathBuf>,
}

impl MultiWriteSession {
  fn new(root: PathBuf, candidates: Vec<PathBuf>) -> Self {
    Self {
      root,
      candidates,
      files: HashMap::new(),
      originals: HashMap::new(),
      created: HashSet::new(),
    }
  }

  fn load(&mut self, path: &Path) -> Result<String> {
    if let Some(content) = self.files.get(path) {
      return Ok(content.clone());
    }
    let content = std::fs::read_to_string(path)?;
    self
      .originals
      .entry(path.to_path_buf())
      .or_insert_with(|| content.clone());
    self.files.insert(path.to_path_buf(), content.clone());
    Ok(content)
  }

  fn peek(&self, path: &Path) -> Option<String> {
    self
      .files
      .get(path)
      .cloned()
      .or_else(|| std::fs::read_to_string(path).ok())
  }

  fn finish(&self, update_all: bool, color: ColorArg) -> Result<usize> {
    let mut paths: Vec<_> = self.files.keys().cloned().collect();
    paths.sort();
    let styles = DiffStyles::from(color);
    let mut changed = 0;
    for path in paths {
      let old = self.originals.get(&path).cloned().unwrap_or_default();
      let new = self.files.get(&path).cloned().unwrap_or_default();
      let is_new = !self.originals.contains_key(&path);
      if old == new && !is_new {
        continue;
      }
      changed += 1;
      if update_all {
        if !is_new {
          write_atomically(&path, &new)?;
        } else {
          write_new_file(&path, &new)?;
        }
      } else {
        let display = path.strip_prefix(&self.root).unwrap_or(&path);
        println!("{}", display.display());
        styles.print_diff(&old, &new, &mut std::io::stdout(), 3)?;
        println!();
      }
    }
    Ok(changed)
  }
}

fn run_seed(
  session: &mut MultiWriteSession,
  rules: &RuleCollection<SgLang>,
  seed: &Seed,
) -> Result<()> {
  let rule = rules
    .get_rule(&seed.rule_id)
    .expect("seed rule must exist in collection");
  let writer = rule
    .multi_writers
    .get(&seed.writer_id)
    .expect("seed writer must exist in rule");

  let discovery = discover(writer, rule, seed, session);
  write_discovered(writer, rule, seed, &discovery, session);
  Ok(())
}

struct VisitedFile {
  path: PathBuf,
  bindings: HashMap<String, String>,
}

struct Discovery {
  files: Vec<VisitedFile>,
  /// Variables merged from the seed and every discovered match.
  /// The first binding of a name wins, mirroring rewriter env pinning.
  vars: HashMap<String, String>,
}

fn discover(
  writer: &MultiWriter,
  rule: &RuleConfig<SgLang>,
  seed: &Seed,
  session: &MultiWriteSession,
) -> Discovery {
  let mut queue = VecDeque::from([(seed.path.clone(), 0usize, seed.vars.clone())]);
  let mut seen = HashSet::new();
  let mut warned = HashSet::new();
  let mut files = vec![];
  let mut vars = seed.vars.clone();
  while let Some((path, depth, bindings)) = queue.pop_front() {
    // Rewriters inherit the enclosing rule environment, but do not inherit
    // match variables recursively. The same rule applies here: seed variables
    // are inherited by every file, while local discovery variables belong to
    // the file that produced them.
    for conflict in merge_keep_first(&mut vars, &bindings) {
      warn_conflict(&mut warned, conflict);
    }
    if seen.len() >= writer.max_files || !seen.insert(path.clone()) {
      continue;
    }
    let Some(lang) = SgLang::from_path(&path) else {
      continue;
    };
    if lang != rule.language {
      continue;
    }
    let Some(content) = session.peek(&path) else {
      continue;
    };
    let grep = lang.ast_grep(content);
    files.push(VisitedFile {
      path: path.clone(),
      bindings: bindings.clone(),
    });

    // Seed matching always starts from the enclosing rule env. Match variables
    // from this file are not passed further down the traversal.
    if depth == 0 {
      for (target, target_vars) in
        collect_search_targets(writer, session, &seed.vars, rule.language)
      {
        for conflict in merge_keep_first(&mut vars, &target_vars) {
          warn_conflict(&mut warned, conflict);
        }
        queue.push_back((target, depth + 1, target_vars));
      }
    }
    if (depth == 0 || writer.recursive) && depth < writer.max_depth {
      let targets = collect_follow_targets(writer, &grep, &seed.vars, &path, &session.root);
      for (target, target_vars) in targets {
        for conflict in merge_keep_first(&mut vars, &target_vars) {
          warn_conflict(&mut warned, conflict);
        }
        queue.push_back((target, depth + 1, target_vars));
      }
    }
  }
  Discovery { files, vars }
}

fn warn_conflict(warned: &mut HashSet<String>, conflict: String) {
  if warned.insert(conflict.clone()) {
    eprintln!("warning: multi-writer variable conflict: {conflict}");
  }
}

fn merge_keep_first(
  target: &mut HashMap<String, String>,
  incoming: &HashMap<String, String>,
) -> Vec<String> {
  let mut conflicts = vec![];
  for (name, value) in incoming {
    match target.get(name) {
      Some(existing) if existing != value => {
        conflicts.push(format!("`{name}` kept as {existing:?}; ignored {value:?}"))
      }
      Some(_) => {}
      None => {
        target.insert(name.clone(), value.clone());
      }
    }
  }
  conflicts
}

fn write_discovered(
  writer: &MultiWriter,
  rule: &RuleConfig<SgLang>,
  seed: &Seed,
  discovery: &Discovery,
  session: &mut MultiWriteSession,
) {
  for file in &discovery.files {
    // Mirror a rewriter match: inherit the seed environment plus the
    // variables captured by the match that discovered this file.
    let vars = &file.bindings;
    let Ok(content) = session.load(&file.path) else {
      continue;
    };
    let Some(lang) = SgLang::from_path(&file.path) else {
      continue;
    };
    if lang != rule.language {
      continue;
    }
    let grep = lang.ast_grep(content.clone());
    let mut edits = if file.path == seed.path {
      collect_seed_fix_edits(rule, &content, seed, lang)
    } else {
      vec![]
    };
    edits.extend(collect_rewrite_edits(writer, &grep, vars));
    if !edits.is_empty() {
      let new_content = apply_edits(&content, edits);
      if new_content != content {
        session.files.insert(file.path.clone(), new_content);
      }
    }
  }

  for create in &writer.create {
    create_file(create, seed, rule.language, discovery, session);
  }
}

fn create_file(
  create: &CreateRule,
  seed: &Seed,
  lang: SgLang,
  discovery: &Discovery,
  session: &mut MultiWriteSession,
) {
  let rendered_path = render_template(&create.path, &discovery.vars, lang);
  let Some(path) = resolve_create_path(&seed.path, &rendered_path, &session.root) else {
    eprintln!(
      "warning: cannot resolve created file path `{}`",
      rendered_path.trim()
    );
    return;
  };
  let exists = session.files.contains_key(&path) || path.exists();
  if exists && !create.overwrite {
    return;
  }
  if path.exists() && !session.created.contains(&path) {
    if let Ok(original) = std::fs::read_to_string(&path) {
      session.originals.entry(path.clone()).or_insert(original);
    }
  } else if !path.exists() {
    session.created.insert(path.clone());
  }
  let content = render_template(&create.template, &discovery.vars, lang);
  session.files.insert(path, content);
}

fn collect_seed_fix_edits(
  rule: &RuleConfig<SgLang>,
  content: &str,
  seed: &Seed,
  lang: SgLang,
) -> Vec<Edit<String>> {
  let Some(fixer) = rule.fixer.first() else {
    return vec![];
  };
  let grep = lang.ast_grep(content);
  let matches: Vec<_> = grep.root().find_all(&rule.matcher).collect();
  let matched = matches
    .iter()
    .find(|matched| matched.text() == seed.text)
    .or_else(|| {
      matches
        .iter()
        .find(|matched| matched.range().start == seed.start)
    });
  matched
    .map(|matched| vec![matched.make_edit(&rule.matcher, fixer)])
    .unwrap_or_default()
}

fn seed_env<'a>(
  bindings: &HashMap<String, String>,
) -> MetaVarEnv<'a, ast_grep_core::tree_sitter::StrDoc<SgLang>> {
  let mut env = MetaVarEnv::new();
  for (name, value) in bindings {
    env.insert_owned_var(name, value);
  }
  env
}

fn collect_rewrite_edits(
  writer: &MultiWriter,
  grep: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<SgLang>>,
  bindings: &HashMap<String, String>,
) -> Vec<Edit<String>> {
  let mut edits = vec![];
  for node in grep.root().dfs() {
    for rewriter in &writer.rewrite {
      let mut env = Cow::Owned(seed_env(bindings));
      if let Some(matched) = rewriter.matcher.match_node_with_env(node.clone(), &mut env) {
        let matched = NodeMatch::new(matched, env.into_owned());
        let fixer = rewriter.fixer.first().expect("rewriter must have a fixer");
        edits.push(matched.make_edit(&rewriter.matcher, fixer));
        break;
      }
    }
  }
  edits
}

fn collect_search_targets(
  writer: &MultiWriter,
  session: &MultiWriteSession,
  bindings: &HashMap<String, String>,
  lang: SgLang,
) -> Vec<(PathBuf, HashMap<String, String>)> {
  if writer.search.is_empty() {
    return vec![];
  }
  let mut targets = vec![];
  for path in &session.candidates {
    let Some(candidate_lang) = SgLang::from_path(path) else {
      continue;
    };
    if candidate_lang != lang {
      continue;
    }
    let Ok(relative) = path.strip_prefix(&session.root) else {
      continue;
    };
    let matching: Vec<_> = writer
      .search
      .iter()
      .filter(|search| search.files.is_match(relative))
      .collect();
    if matching.is_empty() {
      continue;
    }
    let Some(content) = session.peek(path) else {
      continue;
    };
    let grep = lang.ast_grep(content);
    let mut matched_vars = None;
    'rules: for search in matching {
      for node in grep.root().dfs() {
        let mut env = Cow::Owned(seed_env(bindings));
        if let Some(matched) = search.matcher.match_node_with_env(node, &mut env) {
          let matched = NodeMatch::new(matched, env.into_owned());
          let mut vars = bindings.clone();
          let local: HashMap<String, String> = matched.get_env().clone().into();
          vars.extend(local);
          matched_vars = Some(vars);
          break 'rules;
        }
      }
    }
    if let Some(vars) = matched_vars {
      targets.push((path.clone(), vars));
    }
  }
  targets.sort_by(|a, b| a.0.cmp(&b.0));
  targets.dedup_by(|a, b| a.0 == b.0);
  targets
}

fn collect_follow_targets(
  writer: &MultiWriter,
  grep: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<SgLang>>,
  bindings: &HashMap<String, String>,
  from: &Path,
  root: &Path,
) -> Vec<(PathBuf, HashMap<String, String>)> {
  let mut targets = vec![];
  let mut seen = HashSet::new();
  for node in grep.root().dfs() {
    for follow in &writer.follow {
      let Some((path, vars)) = match_follow(follow, node.clone(), bindings, from, root) else {
        continue;
      };
      for target in path {
        if seen.insert(target.clone()) {
          targets.push((target, vars.clone()));
        }
      }
    }
  }
  targets
}

fn match_follow(
  follow: &FollowRule,
  node: ast_grep_core::Node<'_, ast_grep_core::tree_sitter::StrDoc<SgLang>>,
  bindings: &HashMap<String, String>,
  from: &Path,
  root: &Path,
) -> Option<(Vec<PathBuf>, HashMap<String, String>)> {
  let mut env = Cow::Owned(seed_env(bindings));
  let matched = follow.matcher.match_node_with_env(node, &mut env)?;
  let matched = NodeMatch::new(matched, env.into_owned());
  let rendered = follow.path.generate_replacement(&matched);
  let rendered = String::from_utf8_lossy(&rendered);
  let mut vars: HashMap<String, String> = bindings.clone();
  let matched_vars: HashMap<String, String> = matched.get_env().clone().into();
  vars.extend(matched_vars);
  let paths = resolve_target(from, &rendered, follow.resolve, root);
  Some((paths, vars))
}

fn render_template(template: &Fixer, vars: &HashMap<String, String>, lang: SgLang) -> String {
  let dummy = lang.ast_grep("");
  let matched = NodeMatch::new(dummy.root(), seed_env(vars));
  let bytes = template.generate_replacement(&matched);
  String::from_utf8_lossy(&bytes).into_owned()
}

fn resolve_create_path(target: &Path, raw: &str, root: &Path) -> Option<PathBuf> {
  let raw = unquote(raw.trim());
  if raw.is_empty() {
    return None;
  }
  let joined = if Path::new(raw).is_absolute() {
    PathBuf::from(raw)
  } else {
    target.parent().unwrap_or(Path::new(".")).join(raw)
  };
  let normalized = normalize_lexical(&joined)?;
  let mut ancestor = normalized.parent()?;
  while !ancestor.exists() {
    ancestor = ancestor.parent()?;
  }
  let canonical = ancestor.canonicalize().ok()?;
  if !canonical.starts_with(root) {
    return None;
  }
  let suffix = normalized.strip_prefix(ancestor).ok()?;
  Some(canonical.join(suffix))
}

fn normalize_lexical(path: &Path) -> Option<PathBuf> {
  let mut normalized = PathBuf::new();
  for component in path.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        if !normalized.pop() {
          return None;
        }
      }
      Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
        normalized.push(component.as_os_str());
      }
    }
  }
  Some(normalized)
}

fn resolve_target(from: &Path, raw: &str, resolve: FollowResolve, root: &Path) -> Vec<PathBuf> {
  let raw = unquote(raw.trim());
  if raw.is_empty() {
    return vec![];
  }
  let base = match resolve {
    FollowResolve::Relative => from.parent().unwrap_or(Path::new(".")),
    FollowResolve::Module => {
      if raw.starts_with('.') || raw.starts_with('/') {
        from.parent().unwrap_or(Path::new("."))
      } else {
        // Bare package specifiers need a package-manager-aware resolver.
        return vec![];
      }
    }
  };
  let target = base.join(raw);
  let mut ret: Vec<_> = candidates(target)
    .into_iter()
    .filter_map(|candidate| candidate.canonicalize().ok())
    .filter(|candidate| candidate.is_file() && candidate.starts_with(root))
    .collect();
  ret.sort();
  ret.dedup();
  ret
}

fn unquote(raw: &str) -> &str {
  for quote in ['\'', '"', '`'] {
    if raw.len() >= 2 && raw.starts_with(quote) && raw.ends_with(quote) {
      return &raw[1..raw.len() - 1];
    }
  }
  raw
}

const JS_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"];

fn candidates(path: PathBuf) -> Vec<PathBuf> {
  let mut ret = vec![];
  if path.is_file() {
    ret.push(path.clone());
  }
  if path.extension().is_none() {
    for ext in JS_EXTENSIONS {
      ret.push(path.with_extension(ext));
    }
    for ext in JS_EXTENSIONS {
      ret.push(path.join("index").with_extension(ext));
    }
  } else if path.is_dir() {
    for ext in JS_EXTENSIONS {
      ret.push(path.join("index").with_extension(ext));
    }
  } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
    && matches!(ext, "js" | "jsx" | "mjs" | "cjs")
  {
    for replacement in ["ts", "tsx", "mts", "cts"] {
      ret.push(path.with_extension(replacement));
    }
  }
  ret
}

fn apply_edits(old: &str, mut edits: Vec<Edit<String>>) -> String {
  edits.sort_by_key(|edit| (edit.position, edit.deleted_length));
  let mut new = String::with_capacity(old.len());
  let mut cursor = 0;
  for edit in edits {
    if edit.position < cursor || edit.position > old.len() {
      continue;
    }
    let end = edit.position + edit.deleted_length;
    if end > old.len() {
      continue;
    }
    new.push_str(&old[cursor..edit.position]);
    new.push_str(&String::from_utf8_lossy(&edit.inserted_text));
    cursor = end;
  }
  new.push_str(&old[cursor..]);
  new
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use tempfile::tempdir;

  fn test_rule() -> RuleConfig<SgLang> {
    let yaml = r#"
id: test
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
multiFix: [writer]
multiWriters:
  - id: writer
    search:
      - files: ['**/*.ts']
        rule:
          pattern: 'import { $OLD } from $SPEC'
    rewrite:
      - id: rename-ref
        rule:
          pattern: $OLD
        fix: $NEW
"#;
    let mut rules = ast_grep_config::from_yaml_string::<SgLang>(yaml, &GlobalRules::default())
      .expect("test rule should parse");
    rules.pop().expect("test rule should exist")
  }

  fn test_seed(path: PathBuf, text: &str) -> Seed {
    Seed {
      path,
      rule_id: "test".into(),
      writer_id: "writer".into(),
      vars: HashMap::new(),
      start: 0,
      text: text.into(),
    }
  }

  #[test]
  fn warn_conflict_deduplicates_by_message() {
    let mut warned = HashSet::new();
    warn_conflict(&mut warned, "conflict".into());
    warn_conflict(&mut warned, "conflict".into());
    assert_eq!(warned.len(), 1);
  }

  #[test]
  fn merge_keep_first_preserves_existing_values() {
    let mut target = HashMap::from([("A".to_string(), "first".to_string())]);
    let incoming = HashMap::from([
      ("A".to_string(), "second".to_string()),
      ("B".to_string(), "new".to_string()),
    ]);
    let conflicts = merge_keep_first(&mut target, &incoming);
    assert_eq!(target["A"], "first");
    assert_eq!(target["B"], "new");
    assert_eq!(conflicts.len(), 1);
  }

  #[test]
  fn discover_skips_unknown_language_and_missing_content() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let rule = test_rule();
    let writer = &rule.multi_writers["writer"];
    let session = MultiWriteSession::new(root.clone(), vec![]);

    let unknown = test_seed(root.join("missing.unknown"), "");
    assert!(discover(writer, &rule, &unknown, &session).files.is_empty());

    let missing = test_seed(root.join("missing.ts"), "");
    assert!(discover(writer, &rule, &missing, &session).files.is_empty());
  }

  #[test]
  fn write_discovered_skips_missing_unknown_and_wrong_language() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::write(root.join("missing.unknown"), "unknown").unwrap();
    fs::write(root.join("other.rs"), "fn old_name() {}").unwrap();
    let rule = test_rule();
    let writer = &rule.multi_writers["writer"];
    let seed = test_seed(root.join("missing.ts"), "");
    let discovery = Discovery {
      files: vec![
        VisitedFile {
          path: root.join("missing.ts"),
          bindings: HashMap::new(),
        },
        VisitedFile {
          path: root.join("missing.unknown"),
          bindings: HashMap::new(),
        },
        VisitedFile {
          path: root.join("other.rs"),
          bindings: HashMap::new(),
        },
      ],
      vars: HashMap::new(),
    };
    let mut session = MultiWriteSession::new(root, vec![]);
    write_discovered(writer, &rule, &seed, &discovery, &mut session);
  }

  #[test]
  fn collect_search_targets_skips_unusable_candidates() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::write(root.join("other.rs"), "fn old_name() {}").unwrap();
    let rule = test_rule();
    let writer = &rule.multi_writers["writer"];
    let candidates = vec![
      root.join("missing.unknown"),
      root.join("other.rs"),
      root.parent().unwrap().join("outside.ts"),
      root.join("missing.ts"),
    ];
    let session = MultiWriteSession::new(root, candidates);
    let lang = SgLang::from_path("dummy.ts").unwrap();
    let targets = collect_search_targets(writer, &session, &HashMap::new(), lang);
    assert!(targets.is_empty());
  }

  #[test]
  fn collect_seed_fix_edits_falls_back_to_start_offset() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let rule = test_rule();
    let content = "export const oldName = 1;\n";
    let lang = SgLang::from_path("dummy.ts").unwrap();
    let seed = test_seed(root.join("a.ts"), "different text");
    let edits = collect_seed_fix_edits(&rule, content, &seed, lang);
    assert_eq!(edits.len(), 1);
  }

  #[test]
  fn normalize_lexical_handles_curdir_parentdir_and_escape() {
    assert_eq!(
      normalize_lexical(Path::new("./a/../b")).unwrap(),
      PathBuf::from("b")
    );
    assert_eq!(normalize_lexical(Path::new("..")), None);
  }

  #[test]
  fn resolve_create_path_handles_empty_and_absolute_paths() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let target = root.join("src/a.ts");
    assert_eq!(resolve_create_path(&target, "", &root), None);
    let absolute = root.join("generated.ts");
    assert_eq!(
      resolve_create_path(&target, absolute.to_str().unwrap(), &root),
      Some(absolute)
    );
  }

  #[test]
  fn resolve_target_rejects_empty_path() {
    let root = tempdir().unwrap();
    assert!(
      resolve_target(
        &root.path().join("a.ts"),
        "",
        FollowResolve::Relative,
        root.path()
      )
      .is_empty()
    );
  }

  #[test]
  fn candidates_probe_directory_with_extension() {
    let dir = tempdir().unwrap();
    let directory = dir.path().join("generated.bar");
    fs::create_dir_all(&directory).unwrap();
    let index = directory.join("index.ts");
    fs::write(&index, "export const x = 1;\n").unwrap();
    let found = candidates(directory);
    assert!(found.contains(&index));
  }

  #[test]
  fn apply_edits_skips_out_of_bounds_ranges() {
    let edit = Edit {
      position: 1,
      deleted_length: 100,
      inserted_text: b"x".to_vec(),
    };
    assert_eq!(apply_edits("abc", vec![edit]), "abc");
  }
}
