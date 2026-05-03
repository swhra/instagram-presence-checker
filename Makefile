BINARY_NAME = instagram
DESTINATION = $(HOME)/bin/$(BINARY_NAME)

LAUNCHCTL_DIR = $(HOME)/Library/LaunchAgents/
LAUNCHCTL_PLIST = com.user.instagram.plist

TARGET_BIN = target/release/$(BINARY_NAME)

.PHONY: all build install clean

all: build

build:
	cargo build --release

clean:
	cargo clean

install: build
	mv $(TARGET_BIN) $(DESTINATION)
	@echo "Installed to $(DESTINATION). Ensure $(DESTINATION) is in your PATH if you intend to run this program as $(BINARY_NAME); otherwise you can run it explicitly as $(DESTINATION)/$(BINARY_NAME)."
	cp $(LAUNCHCTL_PLIST) $(LAUNCHCTL_DIR)
	launchctl load $(LAUNCHCTL_DIR)/$(LAUNCHCTL_PLIST)
	@echo "Set up Launchd service. All OK. To monitor progress, run: tail -f /tmp/instagram.log"
