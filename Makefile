.PHONY: run build clean apk

run:
	cargo run --release

build:
	cargo build --release

clean:
	cargo clean

# Android APK (arm64). Requires:
#   - the Android SDK layout with build-tools/platforms/ndk under ANDROID_HOME
#   - a release keystore: set KEYSTORE/KEYSTORE_PASS_FILE to your own
#     (keep both outside of version control)
ANDROID_HOME        ?= /home/oem/android
ANDROID_NDK_HOME    ?= $(ANDROID_HOME)/ndk/25.1.8937393
KEYSTORE            ?= $(ANDROID_HOME)/keystores/openmix-release.keystore
KEYSTORE_PASS_FILE  ?= $(ANDROID_HOME)/keystores/openmix-release.pass
APKSIGNER           ?= $(lastword $(sort $(wildcard $(ANDROID_HOME)/build-tools/*/apksigner)))

# cargo-apk 0.10 compiles and signs the cdylib APK correctly, then fails while
# resolving the desktop [[bin]] target ("Bin is not compatible with Cdylib").
# Treat the build as successful only when the freshly signed APK verifies, and
# surface the full log for every other failure.
apk:
	rm -f target/release/apk/openmix.apk target/release/apk/openmix.apk.idsig
	@if ! (ANDROID_HOME=$(ANDROID_HOME) ANDROID_NDK_HOME=$(ANDROID_NDK_HOME) \
		CARGO_APK_RELEASE_KEYSTORE=$(KEYSTORE) \
		CARGO_APK_RELEASE_KEYSTORE_PASSWORD=$$(cat $(KEYSTORE_PASS_FILE)) \
		cargo apk build --release > target/apk-build.log 2>&1); then \
		if [ -f target/release/apk/openmix.apk ] && \
			$(APKSIGNER) verify target/release/apk/openmix.apk >/dev/null 2>&1; then \
			echo "cargo-apk reported a post-signing artifact error; the signed APK is valid."; \
		else \
			cat target/apk-build.log; exit 1; \
		fi; \
	fi
	@$(APKSIGNER) verify target/release/apk/openmix.apk >/dev/null
	@echo "APK at target/release/apk/openmix.apk"
