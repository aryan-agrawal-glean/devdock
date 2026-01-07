#!/usr/bin/env bash
# DevDock Installation Script for Private Repository
# Requires: GitHub CLI (gh) with authentication
# Usage: bash <(gh api repos/askscio/devdock/contents/install.sh --jq .content | base64 -d)
#    or: gh repo clone askscio/devdock && cd devdock && ./install.sh

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

info() {
    echo -e "${BLUE}ℹ${NC} $1" >&2
}

success() {
    echo -e "${GREEN}✓${NC} $1" >&2
}

warning() {
    echo -e "${YELLOW}⚠${NC} $1" >&2
}

error() {
    echo -e "${RED}✗${NC} $1" >&2
    exit 1
}

# Check if gh CLI is available
check_gh_cli() {
    if ! command -v gh >/dev/null 2>&1; then
        error "GitHub CLI (gh) is required but not installed.\nInstall it: https://cli.github.com/"
    fi
    
    # Check if authenticated
    if ! gh auth status >/dev/null 2>&1; then
        error "GitHub CLI is not authenticated.\nRun: gh auth login"
    fi
    
    success "GitHub CLI is authenticated"
}

# Detect OS and architecture
detect_platform() {
    local os=""
    local arch=""
    
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

# Get latest release version
get_latest_version() {
    local version
    version=$(gh release list --repo "$REPO" --limit 1 --json tagName --jq '.[0].tagName')
    
    if [ -z "$version" ]; then
        error "Failed to fetch latest version from GitHub"
    fi
    
    echo "$version"
}

# Download binary using gh CLI
download_binary() {
    local version="$1"
    local platform="$2"
    local asset_name="${BINARY_NAME}-${platform}-${version}"
    local temp_dir
    temp_dir=$(mktemp -d)
    
    info "Downloading DevDock ${version} for ${platform}..."
    
    # Download using gh CLI (works with private repos)
    if ! gh release download "$version" --repo "$REPO" --pattern "$asset_name" --dir "$temp_dir"; then
        error "Failed to download binary. Available assets:"
        gh release view "$version" --repo "$REPO" --json assets --jq '.assets[].name'
        exit 1
    fi
    
    # Rename the downloaded file
    mv "${temp_dir}/${asset_name}" "${temp_dir}/${BINARY_NAME}"
    
    # Download and verify checksums if available
    info "Verifying checksum..."
    if gh release download "$version" --repo "$REPO" --pattern "checksums.txt" --dir "$temp_dir" 2>/dev/null; then
        local expected_checksum
        expected_checksum=$(grep "$asset_name" "${temp_dir}/checksums.txt" | awk '{print $1}')
        
        if [ -n "$expected_checksum" ]; then
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
    
    if [ ! -w "$INSTALL_DIR" ]; then
        needs_sudo=true
        warning "Installation directory ${INSTALL_DIR} requires sudo access"
    fi
    
    chmod +x "${temp_dir}/${BINARY_NAME}"
    
    info "Installing to ${INSTALL_DIR}/${BINARY_NAME}..."
    
    if [ "$needs_sudo" = true ]; then
        if ! command -v sudo >/dev/null 2>&1; then
            error "sudo is required but not found."
        fi
        sudo mv "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    else
        mv "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    fi
    
    rm -rf "$temp_dir"
    
    success "DevDock installed successfully!"
}

# Check if binary is in PATH
check_path() {
    if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
        warning "${INSTALL_DIR} is not in your PATH"
        echo "" >&2
        
        # Detect shell and config file
        local shell_config=""
        if [ -n "$BASH_VERSION" ]; then
            if [ -f "$HOME/.bash_profile" ]; then
                shell_config="$HOME/.bash_profile"
            elif [ -f "$HOME/.bashrc" ]; then
                shell_config="$HOME/.bashrc"
            fi
        elif [ -n "$ZSH_VERSION" ]; then
            shell_config="$HOME/.zshrc"
        else
            # Try to detect from SHELL env var
            case "$SHELL" in
                */zsh)
                    shell_config="$HOME/.zshrc"
                    ;;
                */bash)
                    if [ -f "$HOME/.bash_profile" ]; then
                        shell_config="$HOME/.bash_profile"
                    else
                        shell_config="$HOME/.bashrc"
                    fi
                    ;;
            esac
        fi
        
        if [ -n "$shell_config" ]; then
            echo "Would you like to add ${INSTALL_DIR} to your PATH automatically?" >&2
            read -p "Update $shell_config? [y/N] " -n 1 -r
            echo "" >&2
            
            if [[ $REPLY =~ ^[Yy]$ ]]; then
                # Check if already in config (avoid duplicates)
                if grep -q "export PATH=\"${INSTALL_DIR}:\$PATH\"" "$shell_config" 2>/dev/null; then
                    info "PATH export already exists in $shell_config"
                else
                    echo "" >> "$shell_config"
                    echo "# Added by DevDock installer" >> "$shell_config"
                    echo "export PATH=\"${INSTALL_DIR}:\$PATH\"" >> "$shell_config"
                    success "Updated $shell_config"
                    echo "" >&2
                    warning "Please restart your shell or run:"
                    echo "  source $shell_config" >&2
                fi
            else
                echo "Add it manually to your shell profile:" >&2
                echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2
            fi
        else
            echo "Add it to your shell profile:" >&2
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2
        fi
        echo "" >&2
    fi
}

# Main installation flow
main() {
    echo ""
    echo "╔════════════════════════════════════════╗"
    echo "║       DevDock Installation Script      ║"
    echo "╚════════════════════════════════════════╝"
    echo ""
    
    # Check prerequisites
    check_gh_cli
    
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
            echo "" >&2
            read -p "Reinstall anyway? [y/N] " -n 1 -r
            echo "" >&2
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
        local which_devdock
        which_devdock=$(which devdock)
        
        success "Installed to: ${INSTALL_DIR}/${BINARY_NAME}"
        
        # Check if using the correct binary location
        if [ "$which_devdock" != "${INSTALL_DIR}/${BINARY_NAME}" ]; then
            warning "Found existing devdock at: $which_devdock"
            info "Overriding with new binary..."
            
            if [ -w "$(dirname "$which_devdock")" ]; then
                cp "${INSTALL_DIR}/${BINARY_NAME}" "$which_devdock"
                success "Replaced $which_devdock"
            else
                sudo cp "${INSTALL_DIR}/${BINARY_NAME}" "$which_devdock"
                success "Replaced $which_devdock"
            fi
        fi
        
        # Final verification
        local installed_version
        installed_version=$(devdock --version 2>/dev/null | awk '{print $2}')
        success "Active version: ${installed_version} ($(which devdock))"
    else
        check_path
    fi
    
    echo "" >&2
    echo "════════════════════════════════════════" >&2
    echo "" >&2
    success "Installation complete!"
    echo "" >&2
    info "Get started:"
    echo "  devdock --help" >&2
    echo "  devdock start <profile>" >&2
    echo "" >&2
}

main "$@"

