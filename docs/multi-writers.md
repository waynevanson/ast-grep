# Multi-file rewrites (multi-writers)

A multi-writer turns a single ast-grep rule match into a coordinated rewrite
across several related files. The matched file is the **seed**. Starting from
the seed, a multi-writer can:

- follow references to other files;
- search a set of candidate files for additional references;
- apply one or more rewrites to every visited file, including the seed; and
- create new files from templates after discovery finishes.

A normal `fix` edits only the matched node. A multi-writer keeps the per-file
editing rules in `rewrite` and uses `follow`, `search`, and `create` to describe
which related files should be visited.

## Quick start

Given a project with `sgconfig.yml`:

```yaml
ruleDirs:
  - rules
```

add a rule that renames an exported symbol and its imports:

```yaml
# rules/rename-export.yml
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
```

For these files:

```ts
// a.ts
export const oldName = 1;
```

```ts
// b.ts
import { oldName } from './a';
console.log(oldName);
```

preview the changes with:

```bash
ast-grep multi-write
```

and apply them with:

```bash
ast-grep multi-write --update-all
```

The seed rule's normal `fix`, if present, is applied to the seed match. The
multi-writer's `rewrite` rules are additionally applied to every visited file,
including the seed.

## Command line

```text
ast-grep multi-write [OPTIONS] [PATHS]...
```

By default, `multi-write` loads rules from the project configuration
(`sgconfig.yml`). It walks the requested paths, finds matches for rules that
declare `multiFix`, and runs the referenced multi-writers.

| Option | Description |
| --- | --- |
| `-U`, `--update-all` | Write changes to disk. Without this flag, pending diffs are printed and files are left untouched. |
| `-r`, `--rule <RULE_FILE>` | Load rules from a single rule file instead of the project configuration. |
| `--inline-rules <RULE_TEXT>` | Load rules from inline YAML. Separate multiple rules with `---`. Conflicts with `--rule`. |
| `-c`, `--config <CONFIG_FILE>` | Use a different project configuration file. |
| `[PATHS]...` | Files or directories to search. Defaults to `.`. |
| `--globs <GLOBS>` | Include or exclude paths using gitignore-style globs. |
| `--no-ignore <FILE_TYPE>` | Do not respect selected ignore sources, such as hidden files or VCS ignore files. |
| `--follow` | Follow symbolic links while walking the file system. |
| `-j`, `--threads <NUM>` | Set the approximate number of threads for the initial file walk. |
| `--color <WHEN>` | Control colored diff output. |

Multi-write operates on files on disk. The initial walk supplies both the
seed candidates and the candidate set used by `search` rules.

## Rule configuration

Multi-writers are declared on a normal ast-grep rule:

```yaml
id: my-rule
language: TypeScript
rule:
  pattern: 'export const $NAME = $VALUE'
multiFix:
  - my-writer
multiWriters:
  - id: my-writer
    rewrite:
      - id: rewrite-name
        rule:
          pattern: $NAME
        fix: $NEW
```

- `multiFix` is an optional list of multi-writer ids to run when the rule
  matches. Every listed id must exist in `multiWriters`.
- `multiWriters` is an optional list of named multi-writer definitions. Ids
  must be unique within the rule.
- The snake_case aliases `multi_fix` and `multi_writers` are also accepted.

### Multi-writer fields

| Field | Default | Description |
| --- | --- | --- |
| `id` | required | Unique name for the multi-writer. |
| `follow` | `[]` | Rules that discover related files from the current file. |
| `search` | `[]` | Rules that search the initial candidate file set for references. |
| `create` | `[]` | Files to create from templates after discovery. |
| `rewrite` | required | Rewriters applied to every visited file, including the seed. |
| `recursive` | `false` | Whether `follow` discovery continues from visited files. |
| `maxDepth` | `8` | Maximum traversal depth from the seed. Values are clamped to at least `1`. |
| `maxFiles` | `1000` | Maximum number of files visited for one seed, including the seed. Values are clamped to at least `1`. |

### `follow`

`follow` matches nodes in the current file and renders a path from each match:

```yaml
follow:
  - rule:
      pattern: 'import { $OLD } from $SPEC'
    path: $SPEC
    resolve: module
```

Each follow entry accepts the same core rule fields as a normal rule
(`rule`, `constraints`, `utils`, and `transform`), plus:

| Field | Description |
| --- | --- |
| `path` | Template evaluated with the match environment. It should render to the related file path, such as `$SPEC`. |
| `resolve` | How to interpret the rendered path: `relative` (default) or `module`. |

`relative` resolves the path from the current file's directory. `module`
treats the path as a module specifier: relative specifiers such as `./util` and
`../shared` are resolved with JavaScript/TypeScript extension and index probing
(for example, `./a.js` can resolve to `a.ts`). Bare package specifiers such as
`lodash` are ignored because multi-writers do not perform package-manager
resolution.

A discovered target must exist, be inside the project root, and use the same
language as the rule. Paths that do not meet these conditions are skipped.
Quotes around a rendered path are stripped.

`follow` always runs from the seed. When `recursive` is `true`, it also runs
from every visited file, up to `maxDepth`. Each file is visited at most once per
seed.

### `search`

`search` scans the initial candidate file set for additional matches:

```yaml
search:
  - files: ['**/*.ts']
    rule:
      pattern: 'import { $OLD } from $SPEC'
```

Each search entry accepts the same core rule fields as a normal rule, plus:

| Field | Description |
| --- | --- |
| `files` | Globs for candidate files, relative to the project root. |

Search runs once from the seed and adds matching files at depth `1`. The search
candidate set comes from the initial CLI walk, so command-line paths and ignore
options still apply. Files outside the root, files in another language, and
files that do not match any `files` glob are not searched. If `files` is omitted,
the glob set is empty and no files are considered.

Search rules are tried in order. The first matching rule that finds a match in a
candidate file supplies that file's discovery variables.

### `rewrite`

`rewrite` contains ordinary ast-grep rewriters:

```yaml
rewrite:
  - id: rename-ref
    rule:
      pattern: $OLD
    fix: $NEW
```

Each entry supports `id`, `fix`, and the core rule fields (`rule`,
`constraints`, `utils`, and `transform`). Rewriters can reference variables from
the seed rule and from the discovery match that added the file.

Rewrites are applied to every visited file, including the seed. The file is
walked depth-first; for each node, rewrite entries are tried in order and the
first matching entry produces one edit. Put more specific rewrites before more
general ones.

### `create`

`create` generates new files after discovery and rewriting:

```yaml
create:
  - path: 'generated-$NEW.ts'
    template: "import { $NEW } from $SPEC;\n"
    overwrite: false
```

| Field | Default | Description |
| --- | --- | --- |
| `path` | required | Path template, relative to the seed file. Absolute paths are also accepted when they stay inside the project root. |
| `template` | required | Initial file contents. |
| `overwrite` | `false` | Replace the file if it already exists. |

Both `path` and `template` are rendered with the merged variables from the seed
and discovered files. Missing parent directories are created. Empty paths and
paths that escape the project root produce a warning and are skipped. Created
files are not visited or rewritten after creation.

## Variables and conflicts

Seed variables, including transformed variables, are available throughout the
multi-writer. Variables captured by a `follow` or `search` match are available
to rewrites in the discovered file and to `create` templates, but are not passed
further down the traversal.

When the same variable name is discovered with different values, the first
binding wins and ast-grep prints a warning similar to:

```text
warning: multi-writer variable conflict: `SPEC` kept as "./a"; ignored "./b"
```

Seed variables take precedence over discovered variables, and discovery order
determines which binding is kept.

## Traversal and safety

- Every file visited by a multi-writer must match the rule's language. Files in
  other languages are ignored.
- Each seed runs independently, but edits are accumulated in a shared in-memory
  overlay so later seeds see earlier changes.
- `maxDepth` limits how many edges are followed from the seed. The seed is depth
  `0`, `search` targets start at depth `1`, and follow targets increment the
  depth.
- `maxFiles` limits the number of files visited for one seed, including the
  seed.
- Without `--update-all`, no files are modified.
- With `--update-all`, each file is written atomically. The run as a whole is
  not a transaction, so a failure partway through can leave earlier files
  updated. Use version control and review the dry-run diff first.

## Complete example

This example renames an exported symbol, follows imports to related modules,
searches the project for other imports, and creates a generated file using
variables from the seed and the discovered import:

```yaml
id: rename-and-generate
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
  - rename-and-generate
multiWriters:
  - id: rename-and-generate
    follow:
      - rule:
          pattern: 'import { $OLD } from $SPEC'
        path: $SPEC
        resolve: module
    search:
      - files: ['**/*.ts']
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
    recursive: true
    maxDepth: 8
    maxFiles: 200
```

Run `ast-grep multi-write` to inspect the complete diff, then
`ast-grep multi-write --update-all` to apply it.
