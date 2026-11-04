## v0.11.0 – 2026-11-04

### Features

- Redesigned the web UI: one period switch (7d, 30d, 90d, 1y, All) for tiles, charts and tables, a repo info card, change vs the previous period, gaps shown as hatched bands, and sticky headers; Pico CSS is no longer used (#37)
- Added release download stats: daily snapshots of release asset downloads, a Downloads tile, chart and releases table, and a Downloads column on the home page (#36, #37)
- Added a sync status in the header that shows progress during a long sync (#37)
- Added `GHS_SHOW` to hide repos from the public pages while still collecting them, e.g. `GHS_SHOW='stars:>=25 OR age:<3m'` (#41)
- Added a query syntax for `GHS_FILTER` and `GHS_SHOW` with `OR`, `!`, parentheses, ranges and durations; old comma values like `!fork,vladkens/*` keep working (#41)
- Added `GHS_DISABLE_VERSION_CHECK` to turn off the check for new ghstats releases (#30, by @utoni)
- Embedded the frontend libraries into the binary, so pages no longer load anything from unpkg.com (#32)

### Fixes

- Fixed sync skipping repositories that have pull requests disabled (#34)
- Fixed the star history backfill for repositories you don't administer by using GitHub's star history API, which also needs far fewer requests (#34)
- Fixed missing stars after downtime: the star history is fetched again to fill the gap (#37)
- Fixed pages to start with `<!DOCTYPE html>` and use valid void `meta` tags (#30, by @utoni)

### Improvements

- Schema migrations run in one transaction, so a failed upgrade leaves the database unchanged (#35)
- After upgrading, the first sync fetches the star history of every repo once more to fill old gaps (#37)
- In old comma `GHS_FILTER` values any glob now works (e.g. `vladkens/apigen-*`), and a list of only `!owner/repo` rules means everything except them; an invalid value is logged and ignored (#41)
- `/api/repos` falls back to the default order for an unknown `sort` or `direction` instead of returning an error (#37)
- The new-release notice shows only when a newer release exists, and `main` Docker images report their `git describe` version (#38)

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.10.0...v0.11.0

---

## v0.10.0 – 2026-08-26

### Features

- Added embeddable SVG badges for repository, aggregate, and service metrics
- Added support for rotating GitHub App installation tokens supplied through `GITHUB_TOKEN_FILE`

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.9.0...v0.10.0

---

## v0.9.0 – 2026-06-01

### Features

- Added configurable metrics sync scheduling with `GHS_CRON_SCHEDULE` (#25, by @predkambrij)
- Added Docker Compose support for local builds (#25, by @predkambrij)

### Fixes

- Fixed browser back and forward navigation after changing repository filters (#26, by @larsborn)
- Added a warning for unwritable SQLite databases, making the Docker volume ownership change from v0.8.0 visible instead of silently stopping stats collection

### Docs

- Updated the project overview and use cases
- Documented the multi-architecture Docker build strategy

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.8.0...v0.9.0

---

## v0.8.0 – 2026-04-01

### Breaking Changes

- Changed the Docker image to run as a non-root user, so existing bind-mounted data directories may need an ownership update before stats can be collected

### Features

- Added search and owner filtering to the repositories table (#23, by @larsborn)
- Improved repo metrics UI layout

### Improvements

- Simplified Docker image setup with fewer dependencies
- Migrated to Rust 2024 edition with refreshed dependencies

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.7.1...v0.8.0

---

## v0.7.1 – 2024-12-18

### Fixes

- Fixed colored ANSI escape codes appearing in log output when color should be disabled

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.7.0...v0.7.1

---

## v0.7.0 – 2024-12-18

### Features

- Added logfmt log formatting and support for configuring log level via the `RUST_LOG` environment variable

### Improvements

- Improved graceful shutdown speed

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.6.1...v0.7.0

---

## v0.6.1 – 2024-10-25

### Fixes

- Fixed long URLs overflowing table cells by truncating them
- Fixed sorting by views resetting the selected time period (#19)

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.6.0...v0.6.1

---

## v0.6.0 – 2024-10-21

### Features

- Added support for private repositories (#17)
- Added filters to exclude forked and archived repositories from the listing (#15, by @t0mmili)

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.5.0...v0.6.0

---

## v0.5.0 – 2024-10-12

### Features

- Added separate tracking and display of pull requests distinct from issues (by @t0mmili)

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.4.0...v0.5.0

---

## v0.4.0 – 2024-10-04

### Features

- Added REST API endpoint for accessing repository stats programmatically (#12)

### Fixes

- Fixed default filter state on the repository page
- Added read timeout for GitHub API requests to prevent indefinite hangs

### Docs

- Added API usage documentation to the readme (#12)
- Documented `HOST` and `PORT` configuration options in the readme (#11)

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.3.1...v0.4.0

---

## v0.3.1 – 2024-08-17

### Fixes

- Fixed repository filtering not applying correctly

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.3.0...v0.3.1

---

## v0.3.0 – 2024-08-16

### Features

- Added star history syncing
- Added ability to hide deleted repositories (#10)
- Added repository name filtering (#5)
- Added configurable custom header links (#8)
- Added Docker health check endpoint (#7)
- Added page title shown in the browser tab (#6)
- Added favicon

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.2.0...v0.3.0

---

## v0.2.0 – 2024-08-12

### Features

- Added GitHub organization support (#2)
- Added traffic granularity selector on the repository page
- Added sortable repository table
- Added sorting for popular pages and referring sites tables
- Added new release availability notification

### Improvements

- Removed icon font from the page bundle, reducing initial load size by ~900 KB

**Full Changelog**: https://github.com/vladkens/ghstats/compare/v0.1.0...v0.2.0

---

## v0.1.0 – 2024-07-29

### Features

- Added SQLite storage for persisting traffic data across syncs
- Added GitHub Traffic API integration (clones, views, referring sites, popular paths)
- Added charts for clone and view traffic over time
- Added overall statistics summary on the main page
- Added per-repository totals on the repository page
- Added referring sites and popular pages tracking
- Added scheduled background sync with logging
- Added support for loading more than 100 repositories
- Added Docker image with configurable `HOST` and `PORT` environment variables

**Full Changelog**: https://github.com/vladkens/ghstats/commits/v0.1.0

---
