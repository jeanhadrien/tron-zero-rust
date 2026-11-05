#!/usr/bin/env bash
# One-time IAM setup so this repository's release workflow (.github/workflows/deploy.yml,
# job `server`) can push the game server image and restart the VM, keylessly.
#
#   deploy/gce/grant-ci.sh
#
# It changes who may act as `github-deploy` in the project, so read it first. Idempotent.
# The `github` Workload Identity pool and `github-deploy` already exist (the JS project's
# CI); this adds the Rust repository to them. `github-deploy` already holds
# artifactregistry.writer (image push) and serviceAccountUser.
set -euo pipefail

PROJECT="${PROJECT:-tron-zero-js}"
ZONE="${ZONE:-europe-west1-b}"
VM="${VM:-tron-zero-eu}"
OWNER="${OWNER:-jeanhadrien}"
REPO="${REPO:-tron-zero-rust}"
DEPLOY_SA="github-deploy@${PROJECT}.iam.gserviceaccount.com"
NUMBER="$(gcloud projects describe "$PROJECT" --format='value(projectNumber)')"
POOL="projects/${NUMBER}/locations/global/workloadIdentityPools/github"

# 1. The pool provider only accepts tokens from the JS repository: allow this one too.
gcloud iam workload-identity-pools providers update-oidc github-provider \
  --project "$PROJECT" --location global --workload-identity-pool github \
  --attribute-condition "assertion.repository_owner=='${OWNER}' && (assertion.repository=='${OWNER}/tron-zero-js' || assertion.repository=='${OWNER}/${REPO}')"

# 2. This repository may act as the deploy service account.
gcloud iam service-accounts add-iam-policy-binding "$DEPLOY_SA" --project "$PROJECT" \
  --role roles/iam.workloadIdentityUser \
  --member "principalSet://iam.googleapis.com/${POOL}/attribute.repository/${OWNER}/${REPO}" >/dev/null

# 3. It may change and restart the game server VM, and nothing else in Compute.
gcloud compute instances add-iam-policy-binding "$VM" --zone "$ZONE" --project "$PROJECT" \
  --role roles/compute.instanceAdmin.v1 --member "serviceAccount:${DEPLOY_SA}" >/dev/null

echo "Done: ${OWNER}/${REPO} can deploy as ${DEPLOY_SA}."
