FROM ubuntu:24.04

WORKDIR /opt/graph

COPY static ./static
COPY Config.toml ./Config.toml
COPY target/release/graph /usr/local/bin/graph
COPY docker/entrypoint.sh ./entrypoint.sh

RUN chmod 755 /opt/graph/entrypoint.sh \
    && mkdir -p /graph \
    && chown -R 1000:1000 /graph /opt/graph

ENV TELEGRAPH_CONFIG=/graph/Config.toml \
    TELEGRAPH_DATABASE=/graph/telegraph.db \
    TELEGRAPH_UPLOAD_DIR=/graph/uploads \
    ROCKET_ADDRESS=0.0.0.0 \
    ROCKET_PORT=8000 \
    PUID=1000 \
    PGID=1000

VOLUME ["/graph"]
EXPOSE 8000

ENTRYPOINT ["/opt/graph/entrypoint.sh"]
