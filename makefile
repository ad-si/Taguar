.PHONY: help
help: makefile
	@tail -n +4 makefile | grep ".PHONY"


.PHONY: format
format:
	cargo clippy --fix --allow-dirty
	cargo fmt
	find . -type f -name '*.rs' \
		-exec sed -i -E 's/^([[:space:]]*)\} else/\1}\n\1else/g' {} +


.PHONY: test
test: format
	cargo test -- --show-output


.PHONY: install
install:
	cargo install --path .
