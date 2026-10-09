# AstroFiler web

A browser interface for [AstroFiler](https://github.com/peterbuitho/astrofiler-rs),
meant to run next to the files, for example in Docker on a NAS. Loading,
re-filing, checksums and verification then happen on local disk, and only
web pages cross the network.

It uses the AstroFiler library for all the work, so the catalogue, folder
layout and settings file are the same as the desktop app's.

## What it does

| Page | |
|---|---|
| Images | Search (plus an advanced SQLite condition), filter, sort, group by object or date (the panels of a mosaic are one group); open a file, edit a header field, export, remove from the catalogue or delete from disk (typed confirmation) |
| Load | File a folder into the repository (move, copy or catalogue in place), optionally deleting originals already filed and naming frames that have no target in their header after their folder, and the folder itself once it is empty; processed JPG/PNG/TIFF pictures go along under their own names, telescope thumbnails stay; a nickname in the folder or picture name ("C 7 Spiral Galaxy") is remembered and goes into the object's folder name; nothing is moved or deleted out of the repository's own Light, Stacked, Calibrate and Archive folders; sync the repository; with **Nightly load** switched on in Settings, the incoming folder is moved into the repository every night at the time set there, once nothing has been written to it for 15 minutes |
| Sessions | Create, clear and export sessions |
| Batch | Merge objects, verify, regenerate, folder layout, clean previews |
| Duplicates | Identical files, and deleting the extra copies |
| Mappings | Header values rewritten on import |
| Statistics | Totals and integration time by object, filter, telescope and camera; names that are the same device are added up (configurable) |
| Settings | Repository, incoming folder, conflicts, object names, nicknames, statistics names |
| Log | The latest log lines |

The pages adapt to the screen: the full tables on a desktop, fewer columns
and finger-sized controls on a tablet or phone.

Telescope import and image preview are not here; use the desktop app for
those, then copy the files to the incoming folder on the NAS and load them.

## Run on a Synology NAS

1. Create a folder for the settings and catalogue, e.g. `/volume1/docker/astrofiler`.
2. Edit `compose.yaml`: the share to mount, the `user` that owns it, the
   repository and inbox paths, and the time zone.
3. In Container Manager: Project > Create, point it at the folder holding
   `compose.yaml`. It pulls the image `ntmb/astrofiler-web` from Docker Hub
   (x86-64 only).
4. Open `http://<nas>:8080`.

### First start

The catalogue stores full paths, and the NAS sees the files under a different
path than a desktop that mounts the share. So start with an empty catalogue
and run **Batch > Regenerate catalogue**; it rebuilds the catalogue from the
files in the repository without moving them.

Don't run the desktop app against the same repository with its own catalogue
afterwards: each would move files the other still lists under the old name.

## Settings

Environment variables:

| Variable | Default | |
|---|---|---|
| `ASTROFILER_WEB_LISTEN` | `0.0.0.0:8080` | Address to listen on |
| `ASTROFILER_WEB_ROOT` | `/` | Folder the folder picker is limited to |
| `ASTROFILER_WEB_DESKTOP_ROOT` | | `ASTROFILER_WEB_ROOT` as the desktop mounts it; clicking a file name copies its path in that form |
| `ASTROFILER_WEB_PASSWORD` | | Ask for this password (HTTP basic auth, any user name) |
| `ASTROFILER_WEB_REPO`, `ASTROFILER_WEB_INBOX` | | Repository and incoming folder, used only to create the settings file |
| `ASTROFILER_WEB_VERBOSE` | | Set to log debug messages |
| `ASTROFILER_CONFIG`, `ASTROFILER_DB_PATH` | | Settings file and catalogue (the image sets both to `/data`) |

There is no login unless a password is set, and the pages can delete files.
Keep it on your home network. The password is sent unencrypted over plain
HTTP.

Forms posted from another website are refused, so a page you visit can't
press the buttons for you. Behind a reverse proxy, have the proxy pass the
`Host` header on unchanged, or every button answers "403".

Without a password the pages only answer under an address a home network
gives the NAS: its IP address, a plain name (`nas`) or a `.local` name. Under
any other name (a domain, a VPN name such as `nas.example.ts.net`) they answer
"403" until `ASTROFILER_WEB_PASSWORD` is set, because another website's name
can be pointed at the NAS too.

"Delete previews" removes every JPG and PNG under the folder you give it, so
it refuses the folder picker's root (`ASTROFILER_WEB_ROOT`).

When the container stops, running tasks are asked to stop and the server
waits for them; `compose.yaml` gives it two minutes before Docker ends it.

## Develop

```
ASTROFILER_CONFIG=/tmp/af/astrofiler.ini ASTROFILER_WEB_REPO=/tmp/af/repo \
  ASTROFILER_WEB_LISTEN=127.0.0.1:8080 cargo run
cargo test
docker build -t ntmb/astrofiler-web .
```

The AstroFiler library comes from GitHub at the commit in `Cargo.lock`;
`cargo update -p astrofiler` moves to its latest commit.

## Release

Push a version tag (`git tag v0.3.13 && git push origin v0.3.13`) and the
GitHub Action in `.github/workflows/docker.yml` runs the tests, builds the
image for amd64 and pushes `ntmb/astrofiler-web:0.3.13` and
`:latest` to Docker Hub. It needs two repository secrets (Settings > Secrets
and variables > Actions): `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` (an
access token with write permission, made at hub.docker.com > Account settings
> Personal access tokens).
