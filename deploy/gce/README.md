# Game server on GCP (GCE)

Status: updated 2026-11-02. Verified end to end: image built on Cloud Build,
VM registers with `https://server-manager.tronzero.dev`, a browser joined over
the internet (WebTransport), SIGTERM unregisters cleanly.

The game server needs inbound UDP (5000 raw UDP for desktop, 5001 WebTransport
for browsers), so it runs on a Compute Engine VM, not Cloud Run. The VM runs
Container-Optimized OS; `cloud-init.yaml` installs a systemd unit that pulls
the image named in the instance metadata and runs it with host networking.

## Build the image

From the repo root (about 7-8 min on `E2_HIGHCPU_8`, a few cents per build):

```
gcloud builds submit --project tron-zero-js --region europe-west1 \
  --config cloudbuild.server.yaml --substitutions=_TAG=$(git rev-parse --short HEAD) .
```

## Create or update the VM

```
deploy/gce/create-vm.sh <image-tag> "Tron Zero EU"
```

Idempotent: creates the Artifact Registry repo `tron` and the `tron-zero-game`
firewall rule (udp:5000,5001 to the `tron-zero-game` tag) if missing, then
creates the VM (`tron-zero-eu`, `e2-small`, `europe-west1-b`) or updates its
metadata. After a metadata update, `sudo systemctl restart tron-zero` on the
VM picks up a new image; changes to `cloud-init.yaml` need a VM restart.

The advertised host is the VM's ephemeral external IP, read at every service
start, so no reserved static IP is billed. The IP changes when the VM is
stopped and started; the server re-registers with the new one.

## Official ranked status (no secret)

The unit starts the server with `RANKED_IDENTITY=gcp`: at every registration it asks
the metadata server for an ID token of the VM's service account (`tron-zero-game`,
created by `create-vm.sh`) and the manager checks it against its allowlist. Nothing is
stored in GitHub, Secret Manager or instance metadata. One-time setup:

1. Manager: set `IDENTITY_AUDIENCE=https://server-manager.tronzero.dev` (must equal the
   VM's `tron-manager-url`) and redeploy it.
2. Trust the account:
   `STORE=firestore bun scripts/add-identity.ts tron-zero-game@tron-zero-js.iam.gserviceaccount.com official`
   (in `server-manager/`).
3. An existing VM must switch service account while stopped; `create-vm.sh` prints the
   exact commands when it finds a mismatch. Then `sudo systemctl restart tron-zero`
   (or restart the VM, as `cloud-init.yaml` changed).

Check `curl https://server-manager.tronzero.dev/api/rooms`: the room should say
`"ranked":true,"official":true`. The server logs `Registered as a ranked server` and
warns if the manager only trusts it as a community server. Verified against locally
signed tokens only (`bun run smoke`); the first real token needs a deploy.

## Run and stop

```
gcloud compute instances start tron-zero-eu --zone europe-west1-b --project tron-zero-js
gcloud compute instances stop  tron-zero-eu --zone europe-west1-b --project tron-zero-js
```

Logs: `gcloud compute ssh tron-zero-eu --zone europe-west1-b --project tron-zero-js --command "sudo journalctl -u tron-zero -f"`.

## Cost

- `e2-small` running: about $0.02/hour (~$13/month if left on). Measured load:
  about 5% CPU with 5 bots and no players, 6 MB RAM.
- Stopped: only the 10 GB standard disk, about $0.40/month.
- Egress: billed per GB; a 10-player game at 120 Hz is small, but nothing caps
  it. Raw UDP is unauthenticated, so a spoofed hello can make the server send
  replication to a victim address until the session lease expires. Set a
  billing budget alert before leaving a server up for strangers.
- Cloud Run manager: scales to zero, max 1 instance; pennies.

## Notes

- COS drops inbound traffic at the host firewall by default; the unit opens
  UDP 5000/5001 with `iptables` before starting the container.
- Self-signed WebTransport certificates live at most 14 days; the server
  exits after 12 days at the first moment no human is connected (hard limit
  24 h later) and systemd restarts it with a fresh certificate and hash.
- The old `tron-zero-server` VM (europe-west2-a, from the JS project) is
  unrelated and stopped.
