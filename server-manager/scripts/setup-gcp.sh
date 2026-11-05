#!/usr/bin/env bash
# One-time GCP setup for ranked mode on Cloud Run: Firestore database, its
# composite index, IAM for the service account, and the ticket signing key in
# Secret Manager. Safe to re-run: every step skips what already exists.
#
#   PROJECT=tron-zero-js REGION=europe-west1 bash scripts/setup-gcp.sh
#
# It changes the project (enables APIs, creates a database and a secret, grants
# a role), so read it before running. It does not deploy: the last lines print
# the `gcloud run deploy` flags that switch the service to Firestore.
set -euo pipefail

PROJECT="${PROJECT:-tron-zero-js}"
REGION="${REGION:-europe-west1}"
SERVICE="${SERVICE:-server-manager}"
SECRET="${SECRET:-ticket-signing-key}"

gcloud services enable firestore.googleapis.com secretmanager.googleapis.com --project "$PROJECT"

# Native-mode database in the service's region (one default database per project).
if ! gcloud firestore databases describe --project "$PROJECT" >/dev/null 2>&1; then
  gcloud firestore databases create --project "$PROJECT" --location "$REGION" --type firestore-native
fi

# Leaderboard query: eligible == true ORDER BY rating DESC (see firestore.indexes.json).
if ! gcloud firestore indexes composite list --project "$PROJECT" --format='value(name)' \
  --filter='collectionGroup=ratings' | grep -q .; then
  gcloud firestore indexes composite create --project "$PROJECT" --collection-group ratings \
    --field-config field-path=eligible,order=ascending \
    --field-config field-path=rating,order=descending
fi

# The identity the service runs as (the default compute account unless one was set).
SA="$(gcloud run services describe "$SERVICE" --project "$PROJECT" --region "$REGION" \
  --format='value(spec.template.spec.serviceAccountName)' 2>/dev/null || true)"
if [ -z "$SA" ]; then
  NUMBER="$(gcloud projects describe "$PROJECT" --format='value(projectNumber)')"
  SA="${NUMBER}-compute@developer.gserviceaccount.com"
fi
echo "Service account: $SA"

gcloud projects add-iam-policy-binding "$PROJECT" --member "serviceAccount:$SA" \
  --role roles/datastore.user --condition=None >/dev/null

# Ed25519 key that signs join tickets. Rotating it invalidates tickets in flight
# and the keys game servers cached at registration, so they must re-register.
if ! gcloud secrets describe "$SECRET" --project "$PROJECT" >/dev/null 2>&1; then
  openssl genpkey -algorithm ed25519 | gcloud secrets create "$SECRET" --project "$PROJECT" \
    --replication-policy automatic --data-file=-
fi
gcloud secrets add-iam-policy-binding "$SECRET" --project "$PROJECT" \
  --member "serviceAccount:$SA" --role roles/secretmanager.secretAccessor >/dev/null

cat <<EOF

Done. Deploy the manager with Firestore and the signing key:

  gcloud run deploy $SERVICE --project $PROJECT --region $REGION \\
    --image <image> --min-instances 0 --max-instances 1 --memory 256Mi --cpu 1 \\
    --update-env-vars STORE=firestore,FIRESTORE_PROJECT=$PROJECT \\
    --update-secrets TICKET_SIGNING_KEY=$SECRET:latest

Then issue a key for each ranked game server:

  STORE=firestore FIRESTORE_PROJECT=$PROJECT bun scripts/add-key.ts <keyId> official
EOF
