# msw-vcs

The contract of `msw-vcs`, the read-only tool through which stages read the job's history: `log`, `diff` and `show`, their arguments, output forms, messages and exit codes.

## Where it runs

- Under the runner, `msw-vcs` is on the stage's `PATH`, and the runner knows the job's base and head.
- While dogfooding, it is `.mintswarm/dogfood/msw-vcs`, a shim over git. The shim reads the job's base from `.mintswarm/job/inputs/vcs.json` at the top of the repository, which the dispatching session writes for every stage job:

  ```json
  {"base": "e3ffc0aa4007162b6f631bb0dacd485ed125e966"}
  ```

  `base` is the base commit's full id. Other keys are ignored. The shim never falls back to a branch name: without a usable `vcs.json` every call fails with exit code 4. `head` is the worktree's `HEAD`.
- `.mintswarm/dogfood/msw-vcs-selftest.py` checks the shim against this contract, and runs as the `msw-vcs-selftest` gate. Its cases are data: each is an argv with the exact stdout, stderr and exit code expected, over a fixture repository it builds. The `msw-vcs` tool must give the same stdout, stderr and exit code as the shim on every case.
- The tool only reads. It has no write subcommand, writes no file and fetches nothing.

## Invocation

```text
msw-vcs log [REV | REV..REV] [-- PATH...]
msw-vcs diff [--numstat | --name-only | --raw] REV REV [-- PATH...]
msw-vcs diff [--numstat | --name-only | --raw] --job [-- PATH...]
msw-vcs show REV[:PATH]
```

- `log` with no revision is `log merge-base..head`, the job's own commits. `log REV` is REV and its ancestors. `log A..B` is the commits that are ancestors of B, B included, and not ancestors of A.
- `diff` compares two revisions, the first as old and the second as new. `diff --job` is `diff merge-base head`: the job's change, the span of the landing floor's `<base>...<head>`.
- The output form options and `--job` come before any revision, in any order. At most one output form is given; with none, the form is the patch.
- `A..B` is accepted only by `log`. `diff` takes exactly two revisions or `--job`.
- `show REV` shows a commit. `show REV:PATH` shows a file's bytes, or lists a directory. `show REV:` lists the whole tree.
- Paths come only after `--`, and `show` takes none there.

## Revisions

A revision is exactly one of:

- `base`: the job's base commit;
- `head`: the job's head commit;
- `merge-base`: the single merge base of base and head;
- a full commit id: 40 or 64 lower-case hexadecimal digits, naming a commit that is an ancestor of base or of head (each commit is its own ancestor).

Anything else is refused before the VCS sees it: a branch, a tag, a remote or other ref, `HEAD`, a suffix such as `~1`, `^` or `@{1}`, a search such as `:/text`, an abbreviated or upper-case id, `A...B`, a range with an empty side (`base..`, `..head`), and an argument starting with `-` where a revision is expected. A full id that names no commit, or a commit that is not an ancestor of base or head, is unknown: it gives the same message as a missing path, so whether an object exists outside the job is not revealed.

## Paths

- A path, after `--` or after `REV:`, is literal and relative to the top of the repository, with `/` separators. No pattern or pathspec magic applies: `:(glob)*` is a file named `:(glob)*`.
- A path that starts with `/`, or has an empty, `.` or `..` component (so also a trailing `/` or an empty argument), is refused.
- In `log` and `diff`, a path selects itself and everything under it. A path that names nothing selects nothing and is not an error.
- In `show REV:PATH`, a path that does not exist at REV is unknown.

## Quoting

A path in the output, and an argument quoted in a message, is written verbatim unless it contains a control byte (0x00 to 0x1F, or 0x7F), `"` or `\`. Then it is written in double quotes, with `\a`, `\b`, `\t`, `\n`, `\v`, `\f`, `\r`, `\"` and `\\` for those bytes and a three-digit octal escape such as `\177` for any other control byte. Bytes from 0x80 up are never escaped. This is git's rule under `core.quotePath=false`. An empty argument is written `""`.

## log

One record per commit, newest first: a commit always comes before its parents (git's `--topo-order`). Records are separated by one blank line, and every line ends with a newline. When there is no commit, the output is empty. A record's lines are:

- `commit <id>`;
- one `parent <id>` per parent, in order, and none for a root commit;
- `author <name> <<email>>`;
- `date <YYYY-MM-DDTHH:MM:SSZ>`: the author date, in UTC;
- `subject <line>`: the first line of the message, as it is (an empty message gives `subject ` with nothing after it);
- one `path <status> <path>` per path changed against the first parent, sorted by path bytes. A root commit is compared with the empty tree. `<status>` is `A` (added), `D` (deleted), `M` (content or executable bit changed) or `T` (type changed, for example a file that became a symlink). There are no renames.

With paths after `--`, a commit is listed only if one of its `path` lines is at or under one of them, and only those `path` lines are written.

Example, `msw-vcs log` over the self-test's fixture:

```text
commit 41ddd836432ff16fe18c0a15917ed0ef59c7987f
parent 12195986430b7a0ade77e9e316b3865d2efde1ec
author Ada Author <ada@example.invalid>
date 2026-01-04T10:30:00Z
subject job: add links and odd names
path A café.txt
path T kind
path A link
path A sub
path A "we\"ird\tname.txt"

commit 12195986430b7a0ade77e9e316b3865d2efde1ec
parent 4c1709d267977235959e53b6443919b75993b7b8
author Ada Author <ada@example.invalid>
date 2026-01-03T00:00:00Z
subject job: edit the readme
path M README.md
path A bin.dat
path D gone.txt
path M mode.sh
```

## show

### A commit

`show REV` writes the commit's `log` record. When the message is not empty, a blank line follows, and then the message, one line per message line with a final newline removed: each non-empty line is indented by four spaces, and an empty line stays empty.

Example, `msw-vcs show head`:

```text
commit 41ddd836432ff16fe18c0a15917ed0ef59c7987f
parent 12195986430b7a0ade77e9e316b3865d2efde1ec
author Ada Author <ada@example.invalid>
date 2026-01-04T10:30:00Z
subject job: add links and odd names
path A café.txt
path T kind
path A link
path A sub
path A "we\"ird\tname.txt"

    job: add links and odd names

    The body explains.

      An indented line.
```

### A file

`show REV:PATH`, where PATH is a file at REV, writes the file's bytes exactly, with nothing added. For a symlink, it writes the link's target.

Example, `msw-vcs show base:main.txt`:

```text
main line
```

### A tree

`show REV:` lists every file of the tree, and `show REV:DIR` every file under DIR, at any depth. A gitlink is listed as a file; a directory is not listed itself. `show REV:PATH`, where PATH is a gitlink, lists that gitlink alone. Each line is:

```text
<mode> <id>\t<path>
```

- `<mode>` is `100644` (a file), `100755` (an executable file), `120000` (a symlink) or `160000` (a gitlink, whose id names a commit of another repository).
- `<id>` is the content's full id.
- `<path>` is the full path from the top of the repository, quoted as above.
- Lines are sorted by path bytes.

Example, `msw-vcs show head:` (the separator before each path is a tab):

```text
100644 6c754f2f728c98f04577a049b4d390b3723c89b9	README.md
100644 f7c6699fecb8dc3cf07f3872f99182577594e68b	bin.dat
100644 d66d22773ba1193f6ceaa6344cc4cb4fc04a8849	café.txt
100644 7e2b6439aebf0bb975796f691b3b227d0af43bb5	docs/guide.md
100644 bd93009536360a2d96f2b097ac88b28f1fc8cdb4	keep.txt
120000 1764325aa997b79e6f74da850facef86261812e1	kind
120000 42061c01a1c70097d1e4579f29a5adf40abdec95	link
100755 fab2da2a688bce41c1444443094eb7933ac5670d	mode.sh
160000 0123456789abcdef0123456789abcdef01234567	sub
100644 b04fedbe6c307caf738551314749c0819e238e97	"we\"ird\tname.txt"
```

Example, `msw-vcs show head:docs`:

```text
100644 7e2b6439aebf0bb975796f691b3b227d0af43bb5	docs/guide.md
```

## diff

Every form compares trees with no renames or copies (a moved file is a deletion and an addition), with gitlinks compared by the id they record, with no attributes, external diff drivers or text conversion, and with the landing floor's diff options (`DIFF_OPTIONS` in `.mintswarm/dogfood/landing/floor.py`, without `-z`). Files appear sorted by path bytes. A path that changes type, for example a file that became a symlink, is one entry with status `T` in `--raw`, and appears as a deletion followed by an addition in the patch and is one line in `--numstat` and `--name-only`. The output is empty when nothing differs.

### The patch

The default form is a unified diff with git's extended headers (`new file mode`, `deleted file mode`, `old mode`, `new mode`), `a/` and `b/` prefixes, three lines of context, the myers algorithm and full content ids on the `index` lines. A hunk header's text after `@@ … @@` is git's default function line. A gitlink's change is written as a `Subproject commit <id>` line. A binary file, one with a NUL byte in its first 8000 bytes, is written as `Binary files a/<path> and b/<path> differ`, with `/dev/null` for a missing side. A file without a final newline gets git's `\ No newline at end of file` line.

Example, `msw-vcs diff --job -- README.md bin.dat mode.sh`:

```text
diff --git a/README.md b/README.md
index 89236d9e2520c3e523644f058420856426263128..6c754f2f728c98f04577a049b4d390b3723c89b9 100644
--- a/README.md
+++ b/README.md
@@ -1,2 +1,3 @@
 readme
-line two
+line 2
+line three
diff --git a/bin.dat b/bin.dat
new file mode 100644
index 0000000000000000000000000000000000000000..f7c6699fecb8dc3cf07f3872f99182577594e68b
Binary files /dev/null and b/bin.dat differ
diff --git a/mode.sh b/mode.sh
old mode 100644
new mode 100755
```

### --numstat

One line per changed path: `<added>\t<deleted>\t<path>`, the counts of added and deleted lines. A binary file, which cannot be measured, is `-\t-\t<path>`: the binary marker. A gitlink counts its `Subproject commit` lines. A change of the executable bit alone is `0\t0`.

Example, `msw-vcs diff --numstat --job`:

```text
2	1	README.md
-	-	bin.dat
1	0	café.txt
0	1	gone.txt
1	1	kind
1	0	link
0	0	mode.sh
1	0	sub
1	0	"we\"ird\tname.txt"
```

### --name-only

One changed path per line.

Example, `msw-vcs diff --name-only --job`:

```text
README.md
bin.dat
café.txt
gone.txt
kind
link
mode.sh
sub
"we\"ird\tname.txt"
```

### --raw

One line per changed path:

```text
:<old mode> <new mode> <old id> <new id> <status>\t<path>
```

- The modes are those of `show` trees, with `000000` for a missing side; `160000` on either side marks a gitlink.
- The ids are full content ids, all zeros for a missing side.
- `<status>` is `A`, `D`, `M` or `T`, as in `log`.

Example, `msw-vcs diff --raw --job`:

```text
:100644 100644 89236d9e2520c3e523644f058420856426263128 6c754f2f728c98f04577a049b4d390b3723c89b9 M	README.md
:000000 100644 0000000000000000000000000000000000000000 f7c6699fecb8dc3cf07f3872f99182577594e68b A	bin.dat
:000000 100644 0000000000000000000000000000000000000000 d66d22773ba1193f6ceaa6344cc4cb4fc04a8849 A	café.txt
:100644 000000 420201136f42027c6f971934cc73615082d65160 0000000000000000000000000000000000000000 D	gone.txt
:100644 120000 ac26907f5fe496cd884436dd409fcf59ee074359 1764325aa997b79e6f74da850facef86261812e1 T	kind
:000000 120000 0000000000000000000000000000000000000000 42061c01a1c70097d1e4579f29a5adf40abdec95 A	link
:100644 100755 fab2da2a688bce41c1444443094eb7933ac5670d fab2da2a688bce41c1444443094eb7933ac5670d M	mode.sh
:000000 160000 0000000000000000000000000000000000000000 0123456789abcdef0123456789abcdef01234567 A	sub
:000000 100644 0000000000000000000000000000000000000000 b04fedbe6c307caf738551314749c0819e238e97 A	"we\"ird\tname.txt"
```

## Errors

On an error, stdout is empty and stderr is exactly one line, `msw-vcs: <message>`. The checks run in this order, and the first that fails decides: the arguments, then the job's set-up, then the revisions and paths. So a refused argument is reported even when `vcs.json` is missing. `<arg>` is the whole argument concerned, as given and quoted as above: for `show`, the whole `REV:PATH`; for `log`, the whole range.

| Exit code | Message | When |
| --- | --- | --- |
| 0 | none | success |
| 2 | `usage: msw-vcs log [REV \| REV..REV] [-- PATH...] \| msw-vcs diff [--numstat \| --name-only \| --raw] (REV REV \| --job) [-- PATH...] \| msw-vcs show REV[:PATH]` | no subcommand or an unknown one; the wrong number of revisions; `--job` with a revision or given twice; paths given to `show` |
| 2 | `at most one of --numstat, --name-only and --raw` | more than one output form |
| 2 | `refused revision: <arg>` | a revision that is not one of the four forms above, including a range outside `log` and an option where a revision is expected |
| 2 | `refused path: <arg>` | a path that is absolute or has an empty, `.` or `..` component |
| 3 | `unknown revision or path: <arg>` | a full id that names no commit, or a commit that is not an ancestor of base or head; a path that does not exist at the revision in `show REV:PATH` |
| 4 | `not in a repository` | the working directory is not in a repository |
| 4 | `missing job input .mintswarm/job/inputs/vcs.json` | the shim's job input is missing |
| 4 | `invalid job input .mintswarm/job/inputs/vcs.json` | it is not UTF-8 JSON, not an object, or has no string `base` |
| 4 | `base in .mintswarm/job/inputs/vcs.json is not a full commit id` | `base` is not a full lower-case id of a commit, for example a branch name |
| 4 | `head is not a commit` | the worktree has no `HEAD` commit |
| 4 | `base and head do not have exactly one merge base` | base and head share no history, or have several merge bases |
| 5 | `vcs failure` | the VCS failed in a way none of the above covers |

The table's `\|` is a table escape; the message has a plain `|`. The messages that name `vcs.json` are the shim's alone: under the runner, which holds base and head itself, they do not occur.
