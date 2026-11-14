# Site d'inscription « FAI 1998 » (v34-portail).
# Construit depuis la racine du dépôt : docker compose build (voir compose.yaml).

FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p v34-server --bin v34-portail \
 && cp target/release/v34-portail /usr/local/bin/v34-portail

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/v34-portail /usr/local/bin/v34-portail
# Utilisateur sans privilège par défaut ; compose.yaml prend celui du
# service v34 (pour partager le fichier des comptes avec le central).
USER 65534:65534
EXPOSE 8034
ENTRYPOINT ["/usr/local/bin/v34-portail"]
