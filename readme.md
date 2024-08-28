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

## 🌟 Features

- Collect & store traffic metrics for all your repos
- List of repos and informative dashboard for each
- No React / Next / Postgres etc, just single and small Docker image (20MB) & SQLite

## 🚀 Usage

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

### Github token generation

`ghstats` need Github Token to collect traffic data from API. Token can be obtained with following steps:

1. Go to https://github.com/settings/tokens
2. Generate new token > Generate new token (classic)
3. Enter name, eg: `ghstats`. Scopes: `public_repo`
4. Click genereate token & copy it
5. Save token to `.env` file with name `GITHUB_TOKEN=???`

## How it works?

Every hour `ghstats` loads the list of public repositories and their statistics, and saves the data in SQLite. If at the first startup there is no repositories in the database, synchronization will happen immediately, if `ghstats` is restarted again, synchronization will be performed according to the scheduler. Data is stored per day, re-fetching data for the current day will update existing records in the database.

All public repositories that can be accessed are saved. If you need more detailed configuration – open PR please.

## Configuration

### Host & Port

You can to change default host / port app will run on with `HOST` (default `0.0.0.0`) and `PORT` (default `8080`) environment variables.

### Custom links

If you plan to display your stats publicly, there is an option to add custom links to the header via environment variables, e.g.:

```sh
GHS_CUSTOM_LINKS="Blog|https://medium.com/@vladkens,Github|https://github.com/vladkens,Buy me a coffee|https://buymeacoffee.com/vladkens"
```

### Filter repos

You can filter repos for display (and data collection). You can select a specific org/user or a specific list of repositories. This is configured via the `GHS_FILTER` environment variable. You can use negation in the rules to remove a specific repo or org/user using the `!` symbol. By default all repos show.

_Note: Statistics on previously downloaded repos remain in database, but they are hidden from display._

Usage examples:

```sh
GHS_FILTER=vladkens/macmon,vladkens/ghstats # show only this two repo
GHS_FILTER=vladkens/*,foo-org/bar # show all vladkens repos and one repo from `foo-org`
GHS_FILTER=vladkens/*,!vladkens/apigen-ts # show all vladkens repos except `apigen-ts`
GHS_FILTER=*,!vladkens/apigen-ts,!foo-org/bar # show all repos expect two
```

See example [here](https://github.com/vladkens/ghstats/issues/8).

## 🤝 Contributing

All contributions are welcome! Feel free to open an issue or submit a pull request.

## 🔍 See also

- [repohistory](https://github.com/repohistory/repohistory) – NodeJS application as a service.
