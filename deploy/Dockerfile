# Builds only the status server. The canonical live suite uses real Syncthing
# binaries from the Nix shell; this image is for running the hub service.
FROM docker.io/library/rust:1.90-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p photosync-status

FROM docker.io/library/debian:bookworm-slim
COPY --from=build /src/target/release/photosync-status /usr/local/bin/photosync-status
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/photosync-status"]
CMD ["--config", "/etc/photosync/status.toml", "--token-file", "/run/secrets/token"]
