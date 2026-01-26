This is an open-source alternative to openpilot connect for use with openpilot software.
To see the server in action, go to: https://stable.konik.ai/

## wasmplot (WASM browser log viewer)

A separate frontend in `wasmplot/` lets you parse qlog files (Cap'n Proto) in the browser with WebAssembly and plot series like CarState vEgo.

Quick start:
- Install Rust wasm tooling: `cargo install wasm-pack`
- From `wasmplot/`, run `npm run dev` (or `pnpm dlx vite` after `wasm-pack build --target web`)
- Open the shown URL, paste a qlog URL (.bz2 supported in Chromium via DecompressionStream), and click Plot.

Thank you https://konik.ai for hosting!

# To make your device connect to the server, complete the following steps:
Note. There is no need to unpair the device from comma connect.

* Step 1: SSH into the device.
* Step 2 (Cloned comma devices only): Make sure you generate unique OpenSSL key pairs on the device. You can copy a script from here https://github.com/1okko/openpilot/blob/mr.one/1.sh to generate the keys.

* Step 3: Delete the device dongle ID by running rm /data/params/d/DongleId and rm /persist/comma/dongle_id

If you are running a custom fork of openpilot that already has the code changes required, then you can reboot the device now and scan the qr code on the [website](https://stable.konik.ai/) pair the device.

If you are using a fork that does not have the code changes, you will need to continue with the following steps:

Step 4: export the server urls in launch_openpilot.sh by adding this to that file.
```bash
#!/usr/bin/bash
export API_HOST=https://api.konik.ai
export ATHENA_HOST=wss://athena.konik.ai
# Any other custom launch options here
exec ./launch_chffrplus.sh
```

Step 5: Commit your changes and disable automatic software updates in the openpilot settings (if applicable).
```git commit -a -m "switched to konik server"```

Step 5: Reboot the device and scan the QR code on the [website](https://stable.konik.com/). The QR code must be scanned with the konik website and not comma connect.


# Hosting you own instance (Hardcore)

To get started with hosting your own instance, inspect the docker compose yaml to adjust the volume mount points.
https://github.com/MoreTore/connect-killer/blob/4b9be8252688df5672448b1139da4b4a71c554dc/docker-compose.yml#L3-L53
fill out the .env_template and rename it to .env
https://github.com/MoreTore/connect-killer/blob/4b9be8252688df5672448b1139da4b4a71c554dc/.env_template#L1-L18
create openssl keys for your domain and put them into self_signed_certs folder. See here https://github.com/MoreTore/connect-killer/blob/4b9be8252688df5672448b1139da4b4a71c554dc/src/app.rs#L151-L158
More changes to hard coded values need to be changed to get the frontend working. More work needs to be done to make it easier.

run docker compose up --build

## SMART disk alerts on the host

If you rely on the host's `smartmontools` service (outside Docker) you can keep the configuration under version control with the helper script in `scripts/smartd_mail.sh`. It wraps `/usr/sbin/sendmail` so that it can be referenced from `smartd.conf` without extra arguments (the `-M exec` directive only accepts a single path).

1. Copy the script to your host and make it executable:
   ```bash
   sudo install -m 755 scripts/smartd_mail.sh /usr/local/bin/smartd_mail.sh
   ```
2. Configure `/etc/smartd.conf` to monitor every device and send alerts to `ryleymcc@shaw.ca`:
   ```
   DEFAULT -a -m ryleymcc@shaw.ca -M exec /usr/local/bin/smartd_mail.sh
   DEVICESCAN -a -o on -S on -s (S/../.././02) -s (L/../../6/03)
   ```
   The `DEVICESCAN` line mirrors your current schedule (short self-test daily at 02:00 and long self-test every Saturday at 03:00). Adjust it if you need a different cadence.
3. Reload the daemon so it picks up the syntax-safe configuration:
   ```bash
   sudo systemctl restart smartmontools.service
   sudo systemctl status smartmontools.service --no-pager
   ```
4. Trigger a test email from smartd to verify delivery:
   ```bash
   sudo smartd -c /etc/smartd.conf -q onecheck -M test
   ```

Because the wrapper script handles the `sendmail -t -f ...` invocation, the systemd unit no longer crashes with the "unknown Directive" error and you still receive emails via `msmtp`/`sendmail`.
