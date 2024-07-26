# ghstats

[<img src="https://badgen.net/github/release/vladkens/ghstats" alt="version" />](https://github.com/vladkens/ghstats/releases)
[<img src="https://badgen.net/github/license/vladkens/ghstats" alt="license" />](https://github.com/vladkens/ghstats/blob/main/LICENSE)
[<img src="https://badgen.net/static/-/buy%20me%20a%20coffee/ff813f?icon=buymeacoffee&label" alt="donate" />](https://buymeacoffee.com/vladkens)

<div align="center">
  Self-hosted dashboard for tracking GitHub repos traffic history longer than 14 days.
  <br />
  <br />
  <img src="https://github.com/vladkens/ghstats/blob/assets/preview.png?raw=true" alt="ghstats preview" />
</div>

### 🌟 Features

- Collect & store traffic metrics for all your repos
- List of repos and informative dashboard for each
- No React / Next / Postres etc, just single and small Docker image (20MB) & SQLite

### 🚀 Usage

```sh
docker run -d --env-file .env -p 8080:8080 -v ./data:/app/data --name ghstats ghcr.io/vladkens/ghstats:latest
```

Or Docker Compose:

```yaml
services:
  ghstats:
    image: ghcr.io/vladkens/ghstats:latest
    container_name: ghstats
    restart: always
    environment:
      - GITHUB_TOKEN=???
    env_file: .env # or with .env file
    ports:
      - 8080:8080
    volumes:
      - ./data:/app/data
```

#### Github token generation

`ghstats` need Github Token to collect traffic data from API. Token can be obtained with following steps:

1. Go to https://github.com/settings/tokens
2. Generate new token > Generate new token (classic)
3. Enter name, eg: `ghstats`. Scopes: `public_repo`
4. Click genereate token & copy it
5. Save token to `.env` file with name `GITHUB_TOKEN=???`

### 🤝 Contributing

All contributions are welcome! Feel free to open an issue or submit a pull request.
