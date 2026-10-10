# Release

Push a version tag (`git tag v0.3.16 && git push origin v0.3.16`). GitHub
Action `docker.yml` runs tests, builds the amd64 image, and
pushes `ntmb/astrofiler-web:0.3.16` and `:latest` to Docker Hub. Then it
creates GitHub release `0.3.16` with a link to changes since the last one. Edit
its text on GitHub.

Needs two repository secrets (Settings > Secrets and variables > Actions):
`DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` (access token with write
permission, made at hub.docker.com > Account settings > Personal access
tokens).
