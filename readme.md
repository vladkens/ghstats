# ghstats

[<img src="https://badgen.net/github/release/vladkens/ghstats" alt="version" />](https://github.com/vladkens/ghstats/releases)
[<img src="https://badgen.net/github/license/vladkens/ghstats" alt="license" />](https://github.com/vladkens/ghstats/blob/main/LICENSE)
[<img src="https://badgen.net/static/-/buy%20me%20a%20coffee/ff813f?icon=buymeacoffee&label" alt="donate" />](https://buymeacoffee.com/vladkens)

<div align="center">
  Self-hosted dashboard for tracking GitHub repos traffic history longer than 14 days.
  <br />
  <br />
  <img src=".github/ghstats.png" alt="ghstats preview" />
</div>

### Usage

```sh
docker run -d -p 8080:8080 --name ghstats ghcr.io/vladkens/ghstats:latest
```

### Github token generation

1. Go to https://github.com/settings/tokens
2. Generate new token > Generate new token (classic)
3. Enter name, eg: `ghstats`. Scopes: `public_repo`
4. Click genereate token & copy token
