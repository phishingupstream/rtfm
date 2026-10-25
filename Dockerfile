FROM rust:1.99-bookworm@sha256:59037199c44290f2befcdd58dcc540164763fc296950255aaefeef096a1866b0 AS builder
WORKDIR /src
COPY rust/Cargo.toml rust/Cargo.lock ./
COPY rust/src ./src
ARG VCS_REF=development
ENV RTFM_BUILD_REVISION=${VCS_REF}
RUN --mount=type=cache,id=rtfm-cargo-registry,target=/usr/local/cargo/registry cargo build --release --locked

FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/rtfm /usr/local/bin/rtfm
COPY sources.toml /usr/local/share/rtfm/sources.toml
COPY LICENSE /usr/local/share/licenses/rtfm/LICENSE
ENTRYPOINT ["rtfm"]
CMD ["--help"]
