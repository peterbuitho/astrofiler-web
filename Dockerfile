# Static build on Alpine, so the runtime image is a few megabytes.
FROM rust:1-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM alpine:3
RUN apk add --no-cache tzdata
COPY --from=build /src/target/release/astrofiler-web /usr/local/bin/astrofiler-web
# Settings, catalogue and log all live in /data.
ENV ASTROFILER_CONFIG=/data/astrofiler.ini \
    ASTROFILER_DB_PATH=/data/astrofiler.db \
    XDG_DATA_HOME=/data \
    HOME=/data
VOLUME /data
EXPOSE 8080
CMD ["astrofiler-web"]
