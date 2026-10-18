# The relay as a ~5 MB image with nothing else in it: a static binary, Mozilla's root certificates compiled in.
#   podman build -t bipagents-relay relay/
#   podman run -p 8080:8080 -e RELAY_SEAL_KEYS=… -e APNS_KEY_FILE=/run/secrets/apns.p8 … bipagents-relay
FROM docker.io/library/rust:1.99-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY src ./src
RUN cargo build --release --locked

FROM scratch
COPY --from=build /src/target/release/bipagents-relay /bipagents-relay
USER 65534:65534
EXPOSE 8080
ENTRYPOINT ["/bipagents-relay"]
