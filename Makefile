dev:
	cargo watch -q -w 'src' -x 'run'

docker-build:
	docker build -t ghstats .
	docker images -q ghstats | xargs docker inspect -f '{{.Size}}' | xargs numfmt --to=iec

docker-run:
	docker rm --force ghstats || true
	docker run -d -p 8080:8080 -v ./data:/app/data --env-file .env --name ghstats ghstats
