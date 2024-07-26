dev:
	cargo watch -x 'run'

docker-build:
	docker build -t ghstats .
	docker images -q ghstats | xargs docker inspect -f '{{.Size}}' | xargs numfmt --to=iec

docker-run:
	docker rm --force ghstats || true
	docker run -d -p 8080:8080 -v ./data:/data --env-file .env --name ghstats ghstats
