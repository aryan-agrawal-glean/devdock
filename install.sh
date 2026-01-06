#!/usr/bin/env bash
# DevDock Installation Script
# Usage: curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/install.sh | bash

set -e

# Configuration
REPO="askscio/devdock"
INSTALL_DIR="${DEVDOCK_INSTALL_DIR:-/usr/local/bin}"
BINARY_NAME="devdock"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Helper functions
info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

success() {
    echo -e "${GREEN}✓${NC} $1"
}

warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

error() {
    echo -e "${RED}✗${NC} $1"
    exit 1
}

# Detect OS and architecture
detect_platform() {
    local os=""
    local arch=""
    
    # Detect OS
    case "$(uname -s)" in
        Linux*)
            os="linux"
            ;;
        Darwin*)
            os="macos"
            ;;
        *)
            error "Unsupported operating system: $(uname -s)"
            ;;
    esac
    
    # Detect architecture
    case "$(uname -m)" in
        x86_64|amd64)
            arch="x86_64"
            ;;
        arm64|aarch64)
            if [ "$os" = "macos" ]; then
                arch="arm64"
            else
                error "Linux ARM64 is not currently supported"
            fi
            ;;
        *)
            error "Unsupported architecture: $(uname -m)"
            ;;
    esac
    
    echo "${os}-${arch}"
}

# Get latest release version from GitHub
get_latest_version() {
    local version
    version=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    
    if [ -z "$version" ]; then
        error "Failed to fetch latest version from GitHub"
    fi
    
    echo "$version"
}

# Download and verify binary
download_binary() {
    local version="$1"
    local platform="$2"
    local binary_url="https://github.com/${REPO}/releases/download/${version}/${BINARY_NAME}-${platform}-${version}"
    local checksums_url="https://github.com/${REPO}/releases/download/${version}/checksums.txt"
    local temp_dir
    temp_dir=$(mktemp -d)
    
    info "Downloading DevDock ${version} for ${platform}..."
    
    # Download binary
    if ! curl -fsSL "$binary_url" -o "${temp_dir}/${BINARY_NAME}"; then
        error "Failed to download binary from ${binary_url}"
    fi
    
    # Download checksums
    info "Verifying checksum..."
    if curl -fsSL "$checksums_url" -o "${temp_dir}/checksums.txt" 2>/dev/null; then
        # Extract expected checksum for this platform
        local expected_checksum
        expected_checksum=$(grep "${BINARY_NAME}-${platform}-${version}" "${temp_dir}/checksums.txt" | awk '{print $1}')
        
        if [ -n "$expected_checksum" ]; then
            # Compute actual checksum
            local actual_checksum
            if command -v shasum >/dev/null 2>&1; then
                actual_checksum=$(shasum -a 256 "${temp_dir}/${BINARY_NAME}" | awk '{print $1}')
            elif command -v sha256sum >/dev/null 2>&1; then
                actual_checksum=$(sha256sum "${temp_dir}/${BINARY_NAME}" | awk '{print $1}')
            else
                warning "Cannot verify checksum: shasum/sha256sum not found"
                echo "$temp_dir"
                return 0
            fi
            
            if [ "$expected_checksum" != "$actual_checksum" ]; then
                error "Checksum mismatch!\nExpected: ${expected_checksum}\nActual:   ${actual_checksum}"
            fi
            
            success "Checksum verified"
        else
            warning "Could not find checksum for this platform, skipping verification"
        fi
    else
        warning "Could not download checksums, skipping verification"
    fi
    
    echo "$temp_dir"
}

# Install binary
install_binary() {
    local temp_dir="$1"
    local needs_sudo=false
    
    # Check if we need sudo
    if [ ! -w "$INSTALL_DIR" ]; then
        needs_sudo=true
        warning "Installation directory ${INSTALL_DIR} requires sudo access"
    fi
    
    # Make binary executable
    chmod +x "${temp_dir}/${BINARY_NAME}"
    
    # Install binary
    info "Installing to ${INSTALL_DIR}/${BINARY_NAME}..."
    
    if [ "$needs_sudo" = true ]; then
        if ! command -v sudo >/dev/null 2>&1; then
            error "sudo is required but not found. Please run as root or install to a writable directory."
        fi
        sudo mv "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    else
        mv "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    fi
    
    # Cleanup
    rm -rf "$temp_dir"
    
    success "DevDock installed successfully!"
}

# Check if binary is in PATH
check_path() {
    if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
        warning "⚠️  ${INSTALL_DIR} is not in your PATH"
        echo ""
        echo "Add it to your shell profile:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        echo ""
    fi
}

# Main installation flow
main() {
    echo ""
    echo "╔════════════════════════════════════════╗"
    echo "║       DevDock Installation Script      ║"
    echo "╚════════════════════════════════════════╝"
    echo ""
    
    # Detect platform
    local platform
    platform=$(detect_platform)
    info "Detected platform: ${platform}"
    
    # Get latest version
    local version
    version=$(get_latest_version)
    info "Latest version: ${version}"
    
    # Check if already installed
    if command -v devdock >/dev/null 2>&1; then
        local current_version
        current_version=$(devdock --version 2>/dev/null | awk '{print $2}' || echo "unknown")
        warning "DevDock ${current_version} is already installed"
        
        if [ "$current_version" = "${version#v}" ]; then
            info "You already have the latest version installed"
            echo ""
            read -p "Reinstall anyway? [y/N] " -n 1 -r
            echo ""
            if [[ ! $REPLY =~ ^[Yy]$ ]]; then
                info "Installation cancelled"
                exit 0
            fi
        else
            info "Upgrading from ${current_version} to ${version#v}"
        fi
    fi
    
    # Download binary
    local temp_dir
    temp_dir=$(download_binary "$version" "$platform")
    
    # Install binary
    install_binary "$temp_dir"
    
    # Verify installation
    if command -v devdock >/dev/null 2>&1; then
        local installed_version
        installed_version=$(devdock --version 2>/dev/null | awk '{print $2}')
        success "Installed version: ${installed_version}"
    else
        check_path
    fi
    
    echo ""
    echo "════════════════════════════════════════"
    echo ""
    success "Installation complete!"
    echo ""
    info "Get started:"
    echo "  devdock --help"
    echo "  devdock start <profile>"
    echo ""
    info "Documentation: https://github.com/${REPO}"
    echo ""
}

# Run main function
main "$@"

