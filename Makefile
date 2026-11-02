.PHONY: prepare check test build update

prepare:
	cargo +nightly fmt
	cargo clippy --fix --locked --allow-dirty -- -D warnings
	cargo check --release --locked

check:
	cargo +nightly fmt --check
	cargo clippy --locked -- -D warnings
	cargo check --release --locked

test:
	cargo test --locked

build:
	cargo build --release --locked
	ls -lh target/release/$(shell basename $(CURDIR))

update:
	cargo upgrade -i
	cargo update

.PHONY: vendor docker-build backup

# Refresh assets/vendor to the latest release of each library's major
vendor:
	./scripts/vendor.sh

# Build the Docker image and print its size
docker-build:
	docker build -t ghstats:latest .
	docker images -q ghstats:latest | xargs docker inspect -f '{{.Size}}' | xargs numfmt --to=iec

# Copy the production database from $BACKUP_HOST (default srv) into data/
backup:
	./scripts/backup.sh
