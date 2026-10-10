# AstroFiler web

Browser UI for [AstroFiler](https://github.com/peterbuitho/astrofiler-rs).
Run it next to the files, e.g. in Docker on a NAS. Loading, re-filing,
checksums and verification run on local disk. Only web pages cross the network.

It uses the AstroFiler library for all work. Catalogue, folder layout and
settings file are the same as the desktop app's.

## What it does

| Page | |
|---|---|
| Images | Search (plus advanced SQLite condition), filter, sort, group by object or date (mosaic panels are one group). Open file, edit header field, export, remove from catalogue, or delete from disk (typed confirmation) |
| Load | File a folder into the repository (move, copy or catalogue in place). Options: delete originals already filed; name frames with no header target after their folder; delete the folder once empty. Processed JPG/PNG/TIFF pictures go along under their own names. Telescope thumbnails stay. Nickname in folder or picture name ("C 7 Spiral Galaxy") is remembered and goes into the object's folder name. Nothing is moved or deleted out of the repository's own Light, Stacked, Calibrate and Archive folders. Sync the repository. With **Nightly load** on in Settings, the incoming folder moves into the repository every night at the set time, once nothing was written to it for 15 minutes |
| Sessions | Create, clear, export sessions |
| Batch | Merge objects, verify, regenerate, folder layout, clean previews |
| Duplicates | Find identical files, delete extra copies |
| Mappings | Header values rewritten on import |
| Statistics | Totals and integration time by object, filter, telescope, camera. Names of the same device are added up (configurable) |
| Settings | Repository, incoming folder, conflicts, object names, nicknames, statistics names |
| Log | Latest log lines |

Pages adapt to screen: full tables on desktop, fewer columns and big controls
on tablet or phone.

Telescope import and image preview are not here. Use the desktop app for
those, copy files to the incoming folder on the NAS, then load them.

## Run on a Synology NAS

1. Make a folder for settings and catalogue, e.g. `/volume1/docker/astrofiler`.
2. Edit `compose.yaml`: share to mount, `user` that owns it, repository and
   inbox paths, time zone.
3. In Container Manager: Project > Create, point at the folder holding
   `compose.yaml`. It pulls image `ntmb/astrofiler-web` from Docker Hub
   (x86-64 only).
4. Open `http://<nas>:8080`.

### First start

Catalogue stores full paths. NAS sees files under a different path than a
desktop that mounts the share. So start with an empty catalogue and run
**Batch > Regenerate catalogue**. It rebuilds the catalogue from repository
files without moving them.

Afterwards, never run the desktop app on the same repository with its own
catalogue. Each would move files the other still lists under the old name.

## Settings

Environment variables:

| Variable | Default | |
|---|---|---|
| `ASTROFILER_WEB_LISTEN` | `0.0.0.0:8080` | Address to listen on |
| `ASTROFILER_WEB_ROOT` | `/` | Folder the folder picker cannot leave |
| `ASTROFILER_WEB_DESKTOP_ROOT` | | `ASTROFILER_WEB_ROOT` as the desktop mounts it. Click a file name to copy its path in that form |
| `ASTROFILER_WEB_PASSWORD` | | Ask for this password (HTTP basic auth, any user name) |
| `ASTROFILER_WEB_REPO`, `ASTROFILER_WEB_INBOX` | | Repository and incoming folder. Used only to create the settings file |
| `ASTROFILER_WEB_VERBOSE` | | Set to log debug messages |
| `ASTROFILER_CONFIG`, `ASTROFILER_DB_PATH` | | Settings file and catalogue (image sets both to `/data`) |

No login unless a password is set, and pages can delete files. Keep it on
your home network. Password goes unencrypted over plain HTTP.

Forms posted from another website are refused, so a page you visit cannot
press buttons for you. Behind a reverse proxy, pass the `Host` header on
unchanged, or every button answers "403".

Without a password, pages answer only under an address a home network gives
the NAS: IP address, plain name (`nas`) or `.local` name. Under any other name
(domain, VPN name such as `nas.example.ts.net`) they answer "403" until
`ASTROFILER_WEB_PASSWORD` is set. Another website's name can be pointed at the
NAS too.

"Delete previews" removes every JPG and PNG under the given folder. It refuses
the folder picker's root (`ASTROFILER_WEB_ROOT`).

When the container stops, running tasks are asked to stop and the server waits
for them. `compose.yaml` gives it two minutes before Docker ends it.

## Develop

```
ASTROFILER_CONFIG=/tmp/af/astrofiler.ini ASTROFILER_WEB_REPO=/tmp/af/repo \
  ASTROFILER_WEB_LISTEN=127.0.0.1:8080 cargo run
cargo test
docker build -t ntmb/astrofiler-web .
```

AstroFiler library comes from GitHub at the commit in `Cargo.lock`.
`cargo update -p astrofiler` moves to its latest commit.

## Release

Push a version tag (`git tag v0.3.16 && git push origin v0.3.16`). GitHub
Action `.github/workflows/docker.yml` runs tests, builds the amd64 image, and
pushes `ntmb/astrofiler-web:0.3.16` and `:latest` to Docker Hub. Then it
creates GitHub release `0.3.16` with a link to changes since the last one. Edit
its text on GitHub.

Needs two repository secrets (Settings > Secrets and variables > Actions):
`DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` (access token with write
permission, made at hub.docker.com > Account settings > Personal access
tokens).
