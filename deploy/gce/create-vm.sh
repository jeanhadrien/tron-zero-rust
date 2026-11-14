#!/usr/bin/env bash
# Create (or update the metadata of) a Tron Zero game server VM on GCE.
#
#   deploy/gce/create-vm.sh <image-tag>
#
# The VM runs Container-Optimized OS; cloud-init.yaml starts the server image
# under systemd. Room name, capacity and game mode come from the official config
# built into the image (crates/server/config/official.toml). Stop the VM when nobody plays (it is billed while running):
#   gcloud compute instances stop tron-zero-eu --zone europe-west1-b
set -euo pipefail

TAG="${1:?usage: create-vm.sh <image-tag>}"
PROJECT="${PROJECT:-tron-zero-js}"
ZONE="${ZONE:-europe-west1-b}"
VM="${VM:-tron-zero-eu}"
MACHINE="${MACHINE:-e2-small}"
IMAGE="europe-west1-docker.pkg.dev/${PROJECT}/tron/tron-zero-server:${TAG}"
MANAGER_URL="${MANAGER_URL:-https://server-manager.tronzero.dev}"
HERE="$(cd "$(dirname "$0")" && pwd)"

# The identity the server proves to the manager (RANKED_IDENTITY=gcp). Trust it there with
#   STORE=firestore bun scripts/add-identity.ts "$SA_EMAIL" official
SA_NAME="${SA_NAME:-tron-zero-game}"
SA_EMAIL="${SA_NAME}@${PROJECT}.iam.gserviceaccount.com"

METADATA="tron-image=${IMAGE},tron-manager-url=${MANAGER_URL}"

# Image registry (Docker, europe-west1); Cloud Build pushes the server image here.
if ! gcloud artifacts repositories describe tron --location europe-west1 --project "$PROJECT" >/dev/null 2>&1; then
  gcloud artifacts repositories create tron --repository-format docker --location europe-west1 --project "$PROJECT"
fi

# A dedicated service account: a name the manager can trust, holding only the right to
# pull the server image. (The default compute account holds broad roles.)
if ! gcloud iam service-accounts describe "$SA_EMAIL" --project "$PROJECT" >/dev/null 2>&1; then
  gcloud iam service-accounts create "$SA_NAME" --project "$PROJECT" --display-name "Tron Zero game server"
fi
gcloud artifacts repositories add-iam-policy-binding tron --location europe-west1 --project "$PROJECT" \
  --member "serviceAccount:$SA_EMAIL" --role roles/artifactregistry.reader >/dev/null

# Game ports: raw UDP for desktop clients, WebTransport (QUIC, UDP) for browsers.
if ! gcloud compute firewall-rules describe tron-zero-game --project "$PROJECT" >/dev/null 2>&1; then
  gcloud compute firewall-rules create tron-zero-game --project "$PROJECT" \
    --network default --direction INGRESS --action ALLOW \
    --rules udp:5000,udp:5001 --target-tags tron-zero-game --source-ranges 0.0.0.0/0
fi

if gcloud compute instances describe "$VM" --zone "$ZONE" --project "$PROJECT" >/dev/null 2>&1; then
  gcloud compute instances add-metadata "$VM" --zone "$ZONE" --project "$PROJECT" \
    --metadata "$METADATA" --metadata-from-file user-data="$HERE/cloud-init.yaml"
  CURRENT_SA="$(gcloud compute instances describe "$VM" --zone "$ZONE" --project "$PROJECT" --format='value(serviceAccounts[0].email)')"
  if [ "$CURRENT_SA" != "$SA_EMAIL" ]; then
    echo "NOTE: $VM runs as $CURRENT_SA, not $SA_EMAIL. Switch it (the VM must be stopped):"
    echo "  gcloud compute instances stop $VM --zone $ZONE --project $PROJECT"
    echo "  gcloud compute instances set-service-account $VM --zone $ZONE --project $PROJECT --service-account $SA_EMAIL --scopes https://www.googleapis.com/auth/devstorage.read_only,https://www.googleapis.com/auth/logging.write"
    echo "  gcloud compute instances start $VM --zone $ZONE --project $PROJECT"
  fi
  echo "Metadata updated. New image: sudo systemctl restart tron-zero (on the VM). Unit changes: restart the VM."
else
  gcloud compute instances create "$VM" --zone "$ZONE" --project "$PROJECT" \
    --machine-type "$MACHINE" \
    --image-family cos-stable --image-project cos-cloud \
    --boot-disk-size 10GB --boot-disk-type pd-standard \
    --tags tron-zero-game \
    --service-account "$SA_EMAIL" \
    --scopes https://www.googleapis.com/auth/devstorage.read_only,https://www.googleapis.com/auth/logging.write \
    --metadata "$METADATA" --metadata-from-file user-data="$HERE/cloud-init.yaml"
fi
