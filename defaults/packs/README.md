# Guideline packs

The format of a guideline pack: the files in its directory, the fields of its `pack.yaml`, its templates, and the header of the copy a repository adapts from it.

## Layout

- This file, `defaults/packs/README.md`, is the definition of the format and is not a pack.
- Every directory `defaults/packs/<id>/` is one pack and holds exactly these:
  - `pack.yaml`, the manifest;
  - `conventions.md`, the conventions template, required under that fixed name. It is Markdown whose first line is a `# ` title, and it never starts with the header of an adapted copy;
  - `settings/`, the settings templates (subdirectories are allowed). Each file in it is referenced by at least one `settings` entry.
- A pack has no other files.
- Every file is UTF-8 without a BOM, with LF line endings.

## `pack.yaml`

### YAML rules

- It must load the same under mikefarah yq (`yq -p=yaml -o=json .`) and PyYAML `safe_load`:
  - one document, with a mapping at the top level;
  - no `---` or `...` lines;
  - no anchors, aliases, merge keys (`<<`) or tags;
  - no duplicate keys in any mapping. Loaders silently keep the last one, so this is a review rule.
- Every string value is quoted with `"…"` or `'…'`, on one line, with no block scalars. Quoting is what keeps `"3.10"`, `"yes"`, `"010"`, `"2026-11-22"` and `"{{ package_root }}/x"` as strings.
- Integers are plain decimal: `0` or no leading zero, and no `+`, `_` or exponent.
- Booleans are `true` or `false`.
- No floats and no `null` anywhere. An optional field that is left out takes its default.
- A key not in the tables below, or a value of the wrong type, is an error.

### Top-level fields

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `id` | string matching `[a-z][a-z0-9-]*`, equal to the directory name | yes | none |
| `version` | integer of at least 1 | yes | none |
| `name` | string, the display name (for example `"TypeScript"`) | yes | none |
| `description` | string | no | `""` |
| `base` | string, the id of another pack (see [Base packs](#base-packs)) | no | absent |
| `detect` | mapping (see [`detect`](#detect)) | yes | none |
| `parameters` | list of parameter entries | no | `[]` |
| `settings` | list of settings entries | no | `[]` |
| `gates` | list of gate entries | no | `[]` |

### `detect`

Parameters are not allowed in `detect`.

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `files` | list of strings, globs over tracked paths | no | `[]` |
| `shebangs` | list of strings, interpreter names | no | `[]` |

- The two lists hold at least one entry between them.
- Globs have the semantics of `.mintswarm/dogfood/plan-approval/floor.py`: a glob matches the whole path; `**` matches anything including `/`, `**/` zero or more directories, `*` anything within one segment, `?` one character other than `/`; everything else is literal. So a pack writes `**/*.py`, not `*.py`.
- The interpreter of a tracked file: its first line starts with `#!`, and the rest of that line is split on whitespace. The interpreter is the basename of the first word. If that basename is `env`, the interpreter is the first later word that does not start with `-`.
- A pack is detected when any of its globs matches a tracked path, or when a listed interpreter is the interpreter of any tracked file.

### Parameter entry

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `name` | string matching `[a-z][a-z0-9_]*`, unique in the pack | yes | none |
| `type` | `"string"`, `"integer"` or `"boolean"` | yes | none |
| `default` | a value of `type` | yes | none |
| `description` | string, what the setup session asks the maintainer | yes | none |
| `choices` | non-empty list of values of `type`, with no repeats, containing `default`; not allowed for `boolean` | no | absent (any value of the type) |

- String values (defaults, choices and chosen values) are one line with no control characters.

### Settings entry

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `template` | string, a path relative to the pack directory, under `settings/`, of an existing file | yes | none |
| `target` | string, a repository path; may contain parameters | yes | none |
| `kind` | `"formatter"`, `"linter"`, `"compiler"`, `"toolchain"` or `"editor"` | yes | none |
| `mode` | `"file"` or `"fragment"` | no | `"file"` |
| `section` | string, the section the fragment provides (for example `"tool.black"`, `"workspace.lints"`, `"*.sh"`) | with `"fragment"`; not allowed with `"file"` | absent |
| `optional` | boolean; `true` means the setup session only proposes the setting | no | `false` |
| `alternatives` | list of strings, globs over tracked paths; may contain parameters. A match means the tool is already set up elsewhere (for example `".prettierrc*"` or `"setup.cfg"`) | no | `[]` |

- **Parameters in `target` and `alternatives`:**
  - A placeholder must be a whole directory segment followed by `/`.
  - A value rendered there is a relative path: no leading `/`, no `..` segment, and none of `\`, `*`, `?`, `[` or `]`.
  - After rendering, `.` segments are dropped, so the default `"."` gives a path at the repository root.
  - The result is non-empty and relative, with no empty segment.
  - A consumer without the values, such as the readiness check, reads each placeholder segment as `**`. For example, `{{ package_root }}/tsconfig.json` checks `**/tsconfig.json`.
- **`file` mode:** the rendered template is the whole target file.
- **`fragment` mode:**
  - The target is a TOML, EditorConfig or INI-style file with bracketed section headers.
  - Every section header line in the template is `[<section>]` or `[<section>.<rest>]`; `[[…]]` headers are not allowed.
  - The fragment counts as present when the target has a line, with surrounding whitespace stripped, of either form.
  - The setup session merges fragments: it keeps an existing section and appends a missing one.
- The pair of rendered `target` and `section` is unique within a pack.
- A setting counts as configured when its target exists (for a fragment, with the fragment present) or when any of its alternatives matches a tracked path.

### Gate entry

| Field | Type | Required | Default |
| --- | --- | --- | --- |
| `name` | string matching `<id>-[a-z0-9][a-z0-9-]*`, unique in the pack | yes | none |
| `command` | string; may contain parameters; once rendered, non-empty and splittable by `shlex.split` | yes | none |
| `kind` | `"format"`, `"lint"`, `"typecheck"` or `"test"` | yes | none |
| `timeout` | integer, seconds, at least 1 | no | absent (the runner's default, 1800) |
| `optional` | boolean | no | `false` |

- A gate is copied into the repository's `gates.run` as `name`, the rendered `command`, and `timeout` if given. That is the shape `.mintswarm/dogfood/run-gates.py` and `.mintswarm/dogfood/plan-approval/floor.py` read. `kind` and `optional` stay in the pack.
- A `gates.run` entry counts as the pack's gate when its `name` is the same.

## Templates

- A placeholder is a match of `\{\{ *([a-z][a-z0-9_]*) *\}\}`: spaces only, never a newline.
- Placeholders are allowed in `conventions.md`, in every file under `settings/`, and in `settings[].target`, `settings[].alternatives` and `gates[].command`. They are allowed nowhere else, including `detect` and `section`.
- **Rendering:**
  - One `re.sub` pass over the whole text with that pattern replaces each match with the text of the named value. Replaced text is never scanned again.
  - A string value renders verbatim, an integer in decimal, a boolean as `true` or `false`.
  - "Rendered with defaults" means every parameter takes its `default`.
- **Undeclared names and literal text:**
  - A match whose name is not a parameter declared in the same pack is an error.
  - Text that does not match is literal, for example `${{ secrets.TOKEN }}`, `{{- x }}` and `{{ Name }}`.
  - There is no escape syntax, so a template cannot print a placeholder literally.
- Every declared parameter appears in at least one placeholder.
- There are no transforms on values. A derived form is a separate parameter, for example major and minor integers rendered as `py{{ python_major }}{{ python_minor }}`.

## Base packs

- `base` names an existing pack other than the pack itself, and that pack has no `base` of its own. Nesting is limited to one level, so there are no cycles.
- A derived pack:
  - uses only its own parameters in its templates;
  - does not repeat its base's conventions;
  - is adopted only together with its base.
- The combined settings are the base's entries followed by the derived pack's. A derived entry with the same rendered `target` and `section` as a base entry replaces it.
- Each adapted copy's header names only its own pack. A newer base does not change the derived pack's version.

## Adapted copy

- A repository's adapted copy of a pack lives at `docs/conventions/<id>.md`, where `<id>` is the pack id.
- Line 1 is the header, and the rendered `conventions.md` starts on line 2.

### Header

- The header is exactly `<!-- mintswarm-pack: <id> <version> -->` followed by LF, with single spaces and no trailing whitespace.
- Whoever makes the copy writes the header; it is never stored in the pack.
- The id and version are read with this regular expression, where `first_line` is the file's first line without its `\n`; group 1 is the id and group 2 the version:

  ```python
  re.fullmatch(r"<!-- mintswarm-pack: ([a-z][a-z0-9-]*) ([1-9][0-9]*) -->", first_line)
  ```

- A first line that does not start with `<!-- mintswarm-pack:` means there is no header: the file is a hand-written document, not a pack copy.
- A first line that starts with `<!-- mintswarm-pack:` but does not match is a malformed header. So is a matching one whose id differs from the file name without `.md`.
- Example: `<!-- mintswarm-pack: sample 2 -->` reads as id `sample`, version `2`.

## Versioning and copies

- `version` starts at 1.
- A PR that changes any file under `defaults/packs/<id>/` raises that pack's `version` by exactly one. A change to this README changes no pack.
- A repository may edit its copy and its settings freely. The header keeps the version the copy was adapted from.
- A newer version of a pack reaches a repository only as a PR that updates the copy and its header, never by a silent write. A copy whose version is lower than the pack's is not current.

## Example

A made-up pack, `sample`, at version 2. It exists only in this README; there is no `defaults/packs/sample/` directory.

`defaults/packs/sample/pack.yaml`:

```yaml
id: "sample"
version: 2
name: "Sample"
description: "A made-up language, used only to show the format."
detect:
  files: ["**/*.smp"]
  shebangs: ["sample"]
parameters:
  - name: "min_major"
    type: "integer"
    default: 3
    description: "Which major version of Sample is the oldest the code supports?"
  - name: "min_minor"
    type: "integer"
    default: 9
    description: "Which minor version of that major version is the oldest the code supports?"
  - name: "line_length"
    type: "integer"
    default: 120
    description: "What is the longest line the formatter allows?"
    choices: [80, 100, 120]
  - name: "package_root"
    type: "string"
    default: "."
    description: "Which directory, relative to the repository root, holds the Sample package?"
settings:
  - template: "settings/pyproject.toml"
    target: "{{ package_root }}/pyproject.toml"
    kind: "formatter"
    mode: "fragment"
    section: "tool.sample"
  - template: "settings/sample-lint.toml"
    target: "{{ package_root }}/.sample-lint.toml"
    kind: "linter"
    optional: true
    alternatives: ["{{ package_root }}/setup.cfg"]
gates:
  - name: "sample-format"
    command: "sample fmt --check {{ package_root }}"
    kind: "format"
  - name: "sample-lint"
    command: "sample lint {{ package_root }}"
    kind: "lint"
    timeout: 600
```

`defaults/packs/sample/settings/pyproject.toml`, the fragment template:

```toml
[tool.sample]
line-length = {{ line_length }}
target-version = "{{ min_major }}.{{ min_minor }}"
```

Rendered with defaults, its target is `pyproject.toml` at the repository root, and the fragment counts as present when that file has a `[tool.sample]` line or a `[tool.sample.<rest>]` line. A readiness check without the values looks for `**/pyproject.toml`.

The opening of `defaults/packs/sample/conventions.md`:

```markdown
# Sample conventions

Code supports Sample {{ min_major }}.{{ min_minor }} and later.
```

The first two lines of the adapted copy, `docs/conventions/sample.md`, rendered with defaults:

```markdown
<!-- mintswarm-pack: sample 2 -->
# Sample conventions
```

When adopted, the copied `gates.run` entries are `sample-format` with the command `sample fmt --check .`, and `sample-lint` with the command `sample lint .` and a timeout of 600.
