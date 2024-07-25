dev:
	cargo watch -x 'run'

image:
	docker build -t ghstats .

image-run:
	docker rm --force $(shell docker ps -a -q --filter ancestor=ghstats) || true
	docker run --network=host --name ghstats -d -t ghstats
