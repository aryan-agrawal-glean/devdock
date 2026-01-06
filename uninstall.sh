#!/usr/bin/env bash
# DevDock Uninstallation Script

set -e

# Configuration
BINARY_NAME="devdock"
INSTALL_LOCATIONS=(
    "/usr/local/bin"
    "/usr/bin"
    "$HOME/.local/bin"
    "$HOME/bin"
)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

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
}

# Find all installations of devdock
find_installations() {
    local found=()
    
    for location in "${INSTALL_LOCATIONS[@]}"; do
        if [ -f "${location}/${BINARY_NAME}" ]; then
            found+=("${location}/${BINARY_NAME}")
        fi
    done
    
    # Also check if devdock is in PATH
    if command -v devdock >/dev/null 2>&1; then
        local devdock_path
        devdock_path=$(command -v devdock)
        # Add to found if not already there
        if [[ ! " ${found[@]} " =~ " ${devdock_path} " ]]; then
            found+=("$devdock_path")
        fi
    fi
    
    echo "${found[@]}"
}

# Remove binary
remove_binary() {
    local path="$1"
    local needs_sudo=false
    
    # Check if we need sudo
    if [ ! -w "$(dirname "$path")" ]; then
        needs_sudo=true
    fi
    
    if [ "$needs_sudo" = true ]; then
        info "Removing ${path} (requires sudo)..."
        if ! command -v sudo >/dev/null 2>&1; then
            error "sudo is required but not found"
            return 1
        fi
        sudo rm -f "$path"
    else
        info "Removing ${path}..."
        rm -f "$path"
    fi
    
    success "Removed ${path}"
}

# Clean up state and cache
cleanup_data() {
    local cleaned=false
    
    # DevDock state directory (usually ~/.devdock or /tmp/devdock-*)
    if [ -d "$HOME/.devdock" ]; then
        echo ""
        read -p "Remove DevDock state and cache at $HOME/.devdock? [y/N] " -n 1 -r
        echo ""
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            rm -rf "$HOME/.devdock"
            success "Removed $HOME/.devdock"
            cleaned=true
        fi
    fi
    
    # Temporary DevDock files
    local temp_dirs
    temp_dirs=$(find /tmp -maxdepth 1 -name "devdock-*" -type d 2>/dev/null || true)
    if [ -n "$temp_dirs" ]; then
        echo ""
        info "Found temporary DevDock directories:"
        echo "$temp_dirs"
        echo ""
        read -p "Remove temporary files? [y/N] " -n 1 -r
        echo ""
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            echo "$temp_dirs" | xargs rm -rf
            success "Removed temporary files"
            cleaned=true
        fi
    fi
    
    if [ "$cleaned" = false ]; then
        info "No DevDock data directories found"
    fi
}

# Main uninstallation flow
main() {
    echo ""
    echo "╔════════════════════════════════════════╗"
    echo "║     DevDock Uninstallation Script     ║"
    echo "╚════════════════════════════════════════╝"
    echo ""
    
    # Find installations
    local installations
    installations=($(find_installations))
    
    if [ ${#installations[@]} -eq 0 ]; then
        warning "DevDock is not installed or not found in standard locations"
        echo ""
        info "Checked locations:"
        for location in "${INSTALL_LOCATIONS[@]}"; do
            echo "  - ${location}/${BINARY_NAME}"
        done
        echo ""
        cleanup_data
        exit 0
    fi
    
    # Show found installations
    info "Found DevDock installations:"
    for install in "${installations[@]}"; do
        local version
        if [ -x "$install" ]; then
            version=$("$install" --version 2>/dev/null | awk '{print $2}' || echo "unknown")
        else
            version="unknown"
        fi
        echo "  - ${install} (${version})"
    done
    echo ""
    
    # Confirm uninstallation
    read -p "Uninstall DevDock? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        info "Uninstallation cancelled"
        exit 0
    fi
    
    # Remove all installations
    echo ""
    local removed=0
    for install in "${installations[@]}"; do
        if remove_binary "$install"; then
            ((removed++))
        fi
    done
    
    # Clean up data
    echo ""
    cleanup_data
    
    # Summary
    echo ""
    echo "════════════════════════════════════════"
    echo ""
    if [ $removed -gt 0 ]; then
        success "DevDock uninstalled successfully!"
    else
        warning "No binaries were removed"
    fi
    echo ""
    
    # Verify removal
    if command -v devdock >/dev/null 2>&1; then
        warning "devdock command is still in PATH at: $(command -v devdock)"
        info "You may need to restart your shell or manually remove it"
    fi
}

# Run main function
main "$@"

