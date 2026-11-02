#!/usr/bin/env bash
# Create (or update the metadata of) a Tron Zero game server VM on GCE.
#
#   deploy/gce/create-vm.sh <image-tag> [server-name]
#
# The VM runs Container-Optimized OS; cloud-init.yaml starts the server image
# under systemd. Stop the VM when nobody plays (it is billed while running):
#   gcloud compute instances stop tron-zero-eu --zone europe-west1-b
set -euo pipefail

TAG="${1:?usage: create-vm.sh <image-tag> [server-name]}"
NAME_SHOWN="${2:-Tron Zero EU}"
PROJECT="${PROJECT:-tron-zero-js}"
ZONE="${ZONE:-europe-west1-b}"
VM="${VM:-tron-zero-eu}"
MACHINE="${MACHINE:-e2-small}"
IMAGE="europe-west1-docker.pkg.dev/${PROJECT}/tron/tron-zero-server:${TAG}"
MANAGER_URL="${MANAGER_URL:-https://server-manager.tronzero.dev}"
HERE="$(cd "$(dirname "$0")" && pwd)"

METADATA="tron-image=${IMAGE},tron-server-name=${NAME_SHOWN},tron-max-players=10,tron-manager-url=${MANAGER_URL}"

# Image registry (Docker, europe-west1); Cloud Build pushes the server image here.
if ! gcloud artifacts repositories describe tron --location europe-west1 --project "$PROJECT" >/dev/null 2>&1; then
  gcloud artifacts repositories create tron --repository-format docker --location europe-west1 --project "$PROJECT"
fi

# Game ports: raw UDP for desktop clients, WebTransport (QUIC, UDP) for browsers.
if ! gcloud compute firewall-rules describe tron-zero-game --project "$PROJECT" >/dev/null 2>&1; then
  gcloud compute firewall-rules create tron-zero-game --project "$PROJECT" \
    --network default --direction INGRESS --action ALLOW \
    --rules udp:5000,udp:5001 --target-tags tron-zero-game --source-ranges 0.0.0.0/0
fi

if gcloud compute instances describe "$VM" --zone "$ZONE" --project "$PROJECT" >/dev/null 2>&1; then
  gcloud compute instances add-metadata "$VM" --zone "$ZONE" --project "$PROJECT" \
    --metadata "$METADATA" --metadata-from-file user-data="$HERE/cloud-init.yaml"
  echo "Metadata updated. New image: sudo systemctl restart tron-zero (on the VM). Unit changes: restart the VM."
else
  gcloud compute instances create "$VM" --zone "$ZONE" --project "$PROJECT" \
    --machine-type "$MACHINE" \
    --image-family cos-stable --image-project cos-cloud \
    --boot-disk-size 10GB --boot-disk-type pd-standard \
    --tags tron-zero-game \
    --scopes https://www.googleapis.com/auth/devstorage.read_only,https://www.googleapis.com/auth/logging.write \
    --metadata "$METADATA" --metadata-from-file user-data="$HERE/cloud-init.yaml"
fi
