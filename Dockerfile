# Bowerbird's server with its web app. Notebooks, settings and downloaded
# models live in /data. The sign-in password is the BOWERBIRD_TOKEN the
# container starts with.

FROM node:22-trixie-slim AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci
COPY ui/ ./
RUN npm run build

FROM rust:1.99.1-trixie AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang cmake \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# whisper.cpp otherwise compiles for the CPU of the machine that builds the
# image, which can crash on the one that runs it.
ENV GGML_NATIVE=OFF
RUN scripts/fetch-pdfium.sh \
    && cargo build --release --locked -p bowerbird-server

FROM debian:trixie-slim
# tini forwards `docker stop` to the server, which as process 1 would ignore it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --no-create-home --home-dir /data bowerbird \
    && mkdir /data \
    && chown bowerbird /data
WORKDIR /app
COPY --from=server /src/target/release/bowerbird-server /src/vendor/pdfium/lib/libpdfium.so ./
COPY --from=server /src/vendor/pdfium/LICENSE pdfium/
COPY --from=server /src/vendor/pdfium/licenses pdfium/licenses/
COPY --from=ui /src/ui/build ui/
COPY LICENSE THIRD_PARTY_NOTICES.md ./
COPY licenses licenses/
ENV BOWERBIRD_HOME=/data/notebooks \
    BOWERBIRD_CONFIG=/data/settings.toml \
    XDG_DATA_HOME=/data
USER bowerbird
VOLUME /data
EXPOSE 7477
LABEL org.opencontainers.image.source=https://github.com/jolovicdev/bowerbird \
      org.opencontainers.image.description="Bowerbird, a research notebook that answers from your sources with checked citations" \
      org.opencontainers.image.licenses=MIT
ENTRYPOINT ["tini", "--", "/app/bowerbird-server", "--addr", "0.0.0.0:7477", "--ui", "/app/ui"]
