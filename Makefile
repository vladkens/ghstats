dev:
	cargo watch -q -x 'run'

lint:
	cargo fmt --check
	cargo check --release --locked

update:
	@# cargo install cargo-edit
	cargo upgrade -i

docker-build:
	docker build -t ghstats .
	docker images -q ghstats | xargs docker inspect -f '{{.Size}}' | xargs numfmt --to=iec

docker-run:
	docker rm --force ghstats || true
	docker run -d -p 8080:8080 -v ./data:/app/data --env-file .env --name ghstats ghstats

docker-log:
	docker logs ghstats --follow

gh-cache-clear:
	gh cache delete --all
