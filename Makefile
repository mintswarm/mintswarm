# Developer conveniences. The Makefile holds no versions: each target reads them from their one pin.

.PHONY: required-versions

# Prints each required tool's version, read at run time from the file that pins it.
# Fails, naming the file, when a pin cannot be read. Needs yq, which the gates already require.
required-versions:
	@set -eu; \
	command -v yq >/dev/null 2>&1 || { echo 'required-versions: yq not found; it reads the pins' >&2; exit 1; }; \
	pin() { \
	  v=$$(yq -p "$$2" -o yaml "$$3" "$$1" 2>/dev/null) || v=; \
	  case "$$v" in ''|null) echo "required-versions: cannot read the $$4 pin from $$1" >&2; exit 1;; esac; \
	  printf '%s\n' "$$v"; \
	}; \
	gates=.github/workflows/gates.yml; \
	rust=$$(pin rust-toolchain.toml toml '.toolchain.channel' Rust); \
	nextest=$$(pin $$gates yaml '.jobs[].steps[] | select(.name == "cargo-nextest") | .env.VERSION' cargo-nextest); \
	yqv=$$(pin $$gates yaml '.env.YQ_VERSION' yq); \
	buf=$$(pin web/package.json json '.devDependencies."@bufbuild/buf"' buf); \
	black=$$(pin pyproject.toml toml '.tool.black.required-version' black); \
	node=$$(pin web/.node-version yaml '.' Node); \
	pnpm=$$(pin web/package.json json '.packageManager | select(test("^pnpm@")) | sub("^pnpm@"; "") | sub("\+.*$$"; "")' pnpm); \
	printf '%-14s %-10s %s\n' \
	  Rust "$$rust" rust-toolchain.toml \
	  cargo-nextest "$$nextest" $$gates \
	  yq "$$yqv" $$gates \
	  buf "$$buf" web/package.json \
	  black "$$black" pyproject.toml \
	  Node "$$node" web/.node-version \
	  pnpm "$$pnpm" web/package.json
