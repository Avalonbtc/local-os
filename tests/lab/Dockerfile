FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssh-server sudo screen python3 jq curl util-linux ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /run/sshd /opt/rigdeck/current /var/lib/rigdeck/cache /hive /var/log/miner && chmod 700 /var/lib/rigdeck
COPY runtime/rig-runtime.py /opt/rigdeck/current/rig-runtime.py
COPY tests/lab/entrypoint.sh /entrypoint.sh
RUN printf '#!/bin/sh\nexec python3 /opt/rigdeck/current/rig-runtime.py "$@"\n' > /usr/local/bin/rig-miner && chmod +x /usr/local/bin/rig-miner /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
