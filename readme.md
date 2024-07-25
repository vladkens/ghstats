# ghstats

`ghstats` is open-source & self-hosted dashboard for tracking GitHub repos traffic history longer than 14 days.

### Usage

```sh
docker run -d -p 8080:8080 --name ghstats ghcr.io/vladkens/ghstats:latest
```

### Github token generation

1. Go to https://github.com/settings/tokens
2. Generate new token > Generate new token (classic)
3. Enter name, eg: `ghstats`. Scopes: `public_repo`
4. Click genereate token & copy token
