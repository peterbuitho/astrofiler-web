# AstroFiler web

Browser UI for [AstroFiler](https://github.com/peterbuitho/astrofiler-rs).
Run it next to the files, e.g. in Docker on a NAS. Loading, re-filing,
checksums and verification run on the local disk. Only web pages cross the network.

It uses the AstroFiler library for all work. The catalogue, folder layout and
settings file are the same as the desktop app's.

## What it does

| Page | |
|---|---|
| Images | Search (plus advanced SQLite condition), filter, sort, group by object or date (mosaic panels are one group). Open file, edit header field, export, remove from catalogue, or delete from disk (typed confirmation) |
| Load | File a folder into the repository (move, copy or catalogue in place). Options: delete originals that are already filed, name frames with no header target after their folder, and delete the folder once it is empty. Processed JPG/PNG/TIFF pictures go along under their own names. Telescope thumbnails stay. A nickname in a folder or picture name ("C 7 Spiral Galaxy") goes into the object's folder name, and the app remembers it. Loading never moves or deletes files out of the repository's own Light, Stacked, Calibrate and Archive folders. Sync the repository. With **Nightly load** on in Settings, the incoming folder moves into the repository every night at the set time. It first waits for 15 minutes with no writes to the folder |
| Sessions | Create, clear, export sessions |
| Batch | Merge objects, verify, regenerate, folder layout, clean previews |
| Duplicates | Find identical files, delete extra copies |
| Mappings | Header values rewritten on import |
| Statistics | Totals and integration time by object, filter, telescope, camera. Totals for different names of the same device add up (configurable) |
| Settings | Repository, incoming folder, conflicts, object names, nicknames, statistics names |
| Log | Latest log lines |

Pages adapt to the screen: full tables on desktop, fewer columns and big controls
on tablet or phone.

Telescope import and image preview are not here. Use the desktop app for
those. Then copy the files to the incoming folder on the NAS and load them.

## Run on a Synology NAS

1. Make a folder for the settings and catalogue, e.g. `/volume1/docker/astrofiler`.
2. Edit `compose.yaml`: the share to mount, the `user` that owns it, the
   repository and inbox paths, and the time zone.
3. In Container Manager, open Project > Create and point it at the folder that
   holds `compose.yaml`. It pulls the image `ntmb/astrofiler-web` from Docker Hub
   (x86-64 only).
4. Open `http://<nas>:8080`.

### First start

The catalogue stores full paths. The NAS sees the files under a different path
than a desktop that mounts the share. So start with an empty catalogue and run
**Batch > Regenerate catalogue**. It rebuilds the catalogue from the repository
files and does not move them.

Afterwards, never run the desktop app on the same repository with its own
catalogue. Each app would move files that the other still lists under the old name.

## Settings

Environment variables:

| Variable | Default | |
|---|---|---|
| `ASTROFILER_WEB_LISTEN` | `0.0.0.0:8080` | Address to listen on |
| `ASTROFILER_WEB_ROOT` | `/` | Folder the folder picker cannot leave |
| `ASTROFILER_WEB_DESKTOP_ROOT` | | `ASTROFILER_WEB_ROOT` as the desktop mounts it. Click a file name to copy its path in that form |
| `ASTROFILER_WEB_PASSWORD` | | Ask for this password (HTTP basic auth, any user name) |
| `ASTROFILER_WEB_REPO`, `ASTROFILER_WEB_INBOX` | | Repository and incoming folder. Used only to create the settings file |
| `ASTROFILER_WEB_VERBOSE` | | Set it to log debug messages |
| `ASTROFILER_CONFIG`, `ASTROFILER_DB_PATH` | | Settings file and catalogue (the image sets both to `/data`) |

Pages ask for no login until you set a password, and pages can delete files.
Keep the app on your home network. The password goes unencrypted over plain HTTP.

The server refuses forms posted from another website, so a page you visit
cannot press buttons for you. Behind a reverse proxy, pass the `Host` header on
unchanged, or every button answers "403".

Without a password, pages answer only under an address a home network gives
the NAS: IP address, plain name (`nas`) or `.local` name. Under any other name
(domain, VPN name such as `nas.example.ts.net`) they answer "403" until
you set `ASTROFILER_WEB_PASSWORD`. Another website's owner can point its name
at the NAS too.

"Delete previews" deletes every JPG and PNG under the given folder. It refuses
the folder picker's root (`ASTROFILER_WEB_ROOT`).

When the container stops, the server asks running tasks to stop and waits for
them. `compose.yaml` gives it two minutes before Docker ends it.

## Develop

```
ASTROFILER_CONFIG=/tmp/af/astrofiler.ini ASTROFILER_WEB_REPO=/tmp/af/repo \
  ASTROFILER_WEB_LISTEN=127.0.0.1:8080 cargo run
cargo test
docker build -t ntmb/astrofiler-web .
```

The AstroFiler library comes from GitHub at the commit in `Cargo.lock`.
`cargo update -p astrofiler` moves it to the latest commit.
