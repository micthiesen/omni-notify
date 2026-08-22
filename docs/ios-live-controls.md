# Omni Live for iPad Control Center

Omni Live is a private iOS/iPadOS app with one configurable Control Center control. Add it four times and assign Live Slots 1 through 4. Omni ranks live channels exactly like the home page: primary channels first, then current viewers (or session peak when current viewers are unavailable).

Each control shows the channel currently occupying its slot. Tapping it resolves the slot again and opens the current stream URL directly. If that slot is empty, it opens the Omni home page. If the LAN is briefly unavailable, a previously fetched slot remains usable; with no cache, the tap falls back to the Omni home page.

The app is installed privately from this Mac. There is no App Store release and no TestFlight expiry. A paid Apple Developer membership is required because remote Control Center updates use APNs. A development-signed install normally remains valid for the provisioning profile's lifetime, typically up to one year for a paid team.

Both app targets declare local-network access and a narrow insecure-HTTP ATS exception for `omni.boris`. The app signs every request with a timestamped, single-use HMAC, so the shared secret is never sent over the LAN and a captured request cannot be replayed. Slot responses are not encrypted, so use HTTPS if stream metadata itself must be private. The configuration command rejects other insecure hosts.

Free Personal Team signing and AltStore are not substitutes for the paid membership in this build. They can refresh a seven-day app signature, but a free team cannot provision the Push Notifications capability used for remote Control Center reloads. A free-signed build would therefore lose the feature that keeps labels current when live state changes.

## What is automated

The repository owns the Xcode project source, generated `.xcodeproj`, Swift app and extension, server API, APNs sender, tests, and CLI commands. XcodeGen is only needed when changing project structure; the generated project is checked in for normal installs.

```bash
pnpm ios:doctor       # diagnose Xcode, signing, local config, and devices
pnpm ios:configure    # create private local build configuration
pnpm ios:typecheck    # compile-check both targets and run static control fixtures
pnpm ios:generate     # regenerate the checked-in Xcode project
pnpm ios:build        # signed device build
pnpm ios:build simulator
pnpm ios:test         # simulator unit tests
pnpm ios:install -- DEVICE_IDENTIFIER
```

`Config/Local.xcconfig`, build output, tokens, and Apple keys are ignored by Git.

## One-time Mac setup

The current Xcode installation is missing its root-owned CoreSimulator system component. When sitting at the Mac, run:

```bash
sudo installer \
  -pkg /Applications/Xcode.app/Contents/Resources/Packages/XcodeSystemResources.pkg \
  -target /
xcodebuild -runFirstLaunch
```

Then open Xcode once, go to **Xcode > Settings > Accounts**, sign in with the Apple ID that owns the paid developer membership, and allow Xcode to create an Apple Development certificate. Xcode does not need to be used for subsequent builds or installs.

Find the 10-character Team ID at **developer.apple.com/account > Membership details**.

Create the local app configuration. Omitting `--token` creates a new 64-character secret:

```bash
pnpm ios:configure -- --team ABCDE12345
```

The command prints `IOS_CONTROL_AUTH_TOKEN=...`. Put that exact value in the Omni server environment. To reuse an existing token or change the LAN URL/bundle ID:

```bash
pnpm ios:configure -- \
  --team ABCDE12345 \
  --token EXISTING_LONG_SECRET \
  --server http://omni.boris \
  --bundle-id com.micthiesen.OmniLive
```

Run `pnpm ios:doctor` after setup. Every required line should pass. The signing identity warning can remain until Xcode has finished creating the development certificate.

## One-time APNs setup

In **developer.apple.com/account > Certificates, Identifiers & Profiles**:

1. Confirm the app ID matches `OMNI_BUNDLE_ID` (default `com.micthiesen.OmniLive`). Automatic signing can create it on the first device build.
2. Enable the App Group matching `OMNI_APP_GROUP` (default `group.com.micthiesen.OmniLive`) for the app and control extension.
3. Enable Push Notifications for the control/widget extension target. The extension entitlements request this automatically during an `-allowProvisioningUpdates` build.
4. Under **Keys**, create a key with Apple Push Notifications service access. Download the `.p8` file. Apple only offers the download once.
5. Record the Key ID and Team ID. The same `.p8` key works with development and production APNs.

Keep the key out of Git. Copy it into the persistent Omni volume on boris, for example `/home/michael/compose/volumes/omni-notify/AuthKey_KEYID.p8`, which is `/data/AuthKey_KEYID.p8` inside the container.

Configure the server:

```dotenv
IOS_CONTROL_AUTH_TOKEN=the-same-secret-from-ios-configure
IOS_CONTROL_APNS_TEAM_ID=ABCDE12345
IOS_CONTROL_APNS_KEY_ID=KEYID12345
IOS_CONTROL_APNS_KEY_PATH=/data/AuthKey_KEYID.p8
IOS_CONTROL_BUNDLE_ID=com.micthiesen.OmniLive
IOS_CONTROL_HOME_URL=http://omni.boris
```

All four APNs values must be present to enable remote reloads. With only `IOS_CONTROL_AUTH_TOKEN`, the state API and manual app refresh work, but live-state changes do not proactively refresh Control Center.

The server uses Apple's required control push request:

- push type `controls`
- topic `<bundle ID>.push-type.controls`
- priority `10`
- expiration `0`
- payload `{"aps":{"content-changed":true}}`

These values and the per-configured-control token lifecycle follow Apple's
[Updating controls locally and remotely](https://developer.apple.com/documentation/widgetkit/updating-controls-locally-and-remotely)
documentation.

APNs delivery only tells iPadOS that the control changed. The control extension then fetches its small authenticated slot endpoint over the LAN. Consequently, fresh state is available while the iPad can reach `omni.boris`; cached controls remain visible away from home.

## Install on the iPad

1. Connect the iPad to the Mac once by cable, unlock it, accept **Trust This Computer**, and enable **Settings > Privacy & Security > Developer Mode** if prompted.
2. Confirm it appears:

   ```bash
   xcrun devicectl list devices
   ```

3. Copy its identifier and install:

   ```bash
   pnpm ios:install -- 00008112-DEVICE-IDENTIFIER
   ```

   The command regenerates the project, asks Xcode's CLI to register/sign the device and required capabilities, builds the app plus control extension, and installs the resulting app with `devicectl`.

4. Open **Omni Live** once. The preconfigured URL and token should already be filled in. Tap **Save, Test, and Sync** and confirm four slot rows appear.
5. Open Control Center, touch and hold an empty area, tap **Add a Control**, and add **Omni Live Stream** four times.
6. Configure the four copies as **Live Slot 1**, **Live Slot 2**, **Live Slot 3**, and **Live Slot 4**.
7. Reopen Omni Live and tap **Save, Test, and Sync**. Diagnostics should say four controls were registered.

After this first pairing, installs and updates can generally use the same command while the iPad is paired and visible, including over Wi-Fi if wireless developer connection is enabled.

## Expected experience

- A control's main label is the current channel name. Its status text is the stream title.
- Primary-tier channels always occupy earlier slots than background-tier channels, even when a background channel has more viewers.
- Within a tier, higher current viewer count sorts first. The session peak is used when a platform does not report a current count. Equal counts retain `channels.json` order.
- The live poll runs every 20 seconds for primary channels and every 60 seconds for background channels. After a completed tick changes a displayed slot, Omni sends APNs only to controls configured for that changed slot.
- Adding, removing, or reconfiguring controls replaces that iPad's complete registration set on the server. Registration failures are persisted in the shared app group and retried with backoff; opening the app retries any work the extension could not finish. Stale APNs tokens are deleted when Apple reports them as invalid.
- Network, rate-limit, and APNs 5xx failures get one immediate retry and remain queued for later live-check ticks until delivery succeeds. **Undelivered Controls** in app diagnostics exposes outstanding state without revealing tokens.
- APNs failure never fails the underlying LiveCheck task or interrupts Pushover notifications.

## Final physical-device validation

Do this only after the deferred one-time setup above:

1. `pnpm ios:doctor` passes and lists the iPad.
2. `pnpm ios:test` passes on an installed iPad simulator runtime.
3. `pnpm ios:install -- DEVICE_IDENTIFIER` succeeds with automatic signing.
4. The app's connection test shows the same four slots as the Omni home page.
5. Four configured controls show the expected names and stream titles in Control Center.
6. Tap each live control and confirm Safari or the platform app opens the matching current stream directly.
7. Start or stop a monitored test stream and confirm its affected control refreshes after the next live-check tick without opening Omni Live.
8. Reorder slots by changing relative live viewer counts or tier data and confirm the controls follow the server ordering.
9. Disable Wi-Fi briefly, confirm cached labels remain, and confirm a cached live control still opens its cached URL.
10. Remove one control, reopen Omni Live, sync, and confirm the server logs the reduced registration count.

## Troubleshooting

`CoreSimulator.framework` missing: run the root-owned Xcode package install in the Mac setup section. `xcodebuild -runFirstLaunch` alone may report success without installing that package.

No signing identity: sign into Xcode Settings once. If the team is present but the build cannot create profiles, rerun the install command while online; it passes `-allowProvisioningUpdates` and device registration flags.

Control exists but never changes: open Omni Live and sync. If the registered count is correct, check Omni logs for `IOSControls` warnings and verify the `.p8` path, Team ID, Key ID, bundle ID, and `development` APNs environment.

Control says old channel away from home: expected for the LAN-only server. APNs can arrive anywhere, but the extension cannot fetch `omni.boris` until it rejoins the home network.

HTTP 401: the token compiled into `Config/Local.xcconfig` does not match `IOS_CONTROL_AUTH_TOKEN` on the server. Rerun `pnpm ios:configure` with the correct token, then reinstall.
