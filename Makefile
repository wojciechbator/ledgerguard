CARGO ?= cargo

.PHONY: fmt lint test check deploy deploy-home

fmt:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --locked --all-targets --all-features -- -D warnings

test:
	$(CARGO) test --locked --all-targets --all-features

check: fmt lint test

deploy:
	bash scripts/deploy.sh

deploy-home:
	bash scripts/deploy-home.sh
