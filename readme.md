# ghstats

<div align="center">

Self-hosted dashboard for tracking GitHub repos traffic history longer than 14 days.

[<img src="https://badgen.net/github/release/vladkens/ghstats" alt="version" />](https://github.com/vladkens/ghstats/releases)
[<img src="https://badgen.net/github/license/vladkens/ghstats" alt="license" />](https://github.com/vladkens/ghstats/blob/main/LICENSE)
[<img src="https://badgen.net/static/-/buy%20me%20a%20coffee/ff813f?icon=buymeacoffee&label" alt="donate" />](https://buymeacoffee.com/vladkens)

</div>

<div align="center">
  <img src="https://github.com/vladkens/ghstats/blob/assets/preview.png?raw=true" alt="preview" />
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

### API endpoint

You have the ability to get collected data by `ghstats` via API. At the moment there is only one method available to get all repos list – if you need other data – open PR, please. `GHS_API_TOKEN` environment variable must be set for the API to work. All API calls if protected by `x-api-token` header, which should be same with `GHS_API_TOKEN` variable. CORS is enabled for all hosts, so you can access API from personal pages.

#### Endpoints

`/api/repos` – will return list of all repos and overall metrics. Data returted in JSON format. Usage example:

```sh
curl -H "x-api-token:1234" http://127.0.0.1:8080/api/repos
```

```json
{
  "total_count": 20,
  "total_stars": 1000,
  "total_forks": 200,
  "total_views": 20000,
  "total_clones": 500,
  "items": [
    {
      "id": 833875266,
      "name": "vladkens/ghstats",
      "description": "🤩📈 Self-hosted dashboard for tracking GitHub repos traffic history longer than 14 days.",
      "date": "2024-09-08T00:00:00Z",
      "stars": 110,
      "forks": 1,
      "watchers": 110,
      "issues": 5,
      "prs": 1,
      "clones_count": 90,
      "clones_uniques": 45,
      "views_count": 1726,
      "views_uniques": 659
    }
    // ...
  ]
}
```

## 🤝 Contributing

All contributions are welcome! Feel free to open an issue or submit a pull request.

## 🔍 See also

- [repohistory](https://github.com/repohistory/repohistory) – NodeJS application as a service.
