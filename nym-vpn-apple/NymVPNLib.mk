#
# NymVPNLib build script for Xcode
#

# Build dir passed by xcodebuild
BUILD_DIR ?= $(error BUILD_DIR must be set)

NYM_VPN_CORE_DIR := $(CURDIR)/../nym-vpn-core
NYM_VPN_SWIFT_PACKAGE_DIR := $(CURDIR)/../nym-vpn-core/crates/nym-vpn-lib/NymVPNLib

# Add homebrew and cargo to PATH
export PATH := $(PATH):$(HOME)/.cargo/bin:/opt/homebrew/bin

build:
	make -C $(NYM_VPN_CORE_DIR) -f iOS.mk; \
	echo "Copying framework to build directory: $(BUILD_DIR)"; \
	cp -R $(NYM_VPN_SWIFT_PACKAGE_DIR) $(BUILD_DIR)

clean:
	make -C $(NYM_VPN_CORE_DIR) -f iOS.mk clean
