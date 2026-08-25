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
