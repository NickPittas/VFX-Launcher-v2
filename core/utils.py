"""
Utilities Module

Purpose:
    Contains general-purpose helper functions, regex parsers, timestamp handling,
    path manipulation, and common logic used throughout the application.

Requirements:
    - Reusable, modular functions
    - Thoroughly commented
"""

import os
import re
import logging
from datetime import datetime
import time

# Configure logger
logger = logging.getLogger(__name__)

# Regular expression for extracting version numbers (_v###)
VERSION_REGEX = re.compile(r'_v(\d+)', re.IGNORECASE)


def setup_logger(log_file, log_level=logging.INFO):
    """
    Configure the application logger.
    
    Args:
        log_file (str): Path to log file
        log_level (int): Logging level
    """
    # Create directory for log file if it doesn't exist
    log_dir = os.path.dirname(os.path.abspath(log_file))
    os.makedirs(log_dir, exist_ok=True)
    
    # Configure root logger
    logger = logging.getLogger()
    logger.setLevel(log_level)
    
    # Remove existing handlers
    for handler in logger.handlers[:]:
        logger.removeHandler(handler)
    
    # Create file handler
    file_handler = logging.FileHandler(log_file)
    file_formatter = logging.Formatter(
        '%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )
    file_handler.setFormatter(file_formatter)
    logger.addHandler(file_handler)
    
    # Create console handler
    console_handler = logging.StreamHandler()
    console_formatter = logging.Formatter(
        '%(levelname)s: %(message)s'
    )
    console_handler.setFormatter(console_formatter)
    logger.addHandler(console_handler)
    
    logger.info("Logger initialized")


def extract_version(filename):
    """
    Extract version number from filename using regex.
    Looks for _v### pattern in the filename.
    
    Args:
        filename (str): Filename to extract version from
        
    Returns:
        str: Version number as string, or empty string if not found
    """
    match = VERSION_REGEX.search(filename)
    if match:
        return match.group(1)
    return ""


def get_base_filename(filename):
    """
    Get base filename without version number.
    
    Args:
        filename (str): Filename with version
        
    Returns:
        str: Base filename without version
    """
    # Remove extension
    base = os.path.splitext(filename)[0]
    
    # Remove version suffix
    base = re.sub(r'_v\d+$', '', base)
    
    return base


def format_timestamp(timestamp):
    """
    Format a timestamp for display.
    
    Args:
        timestamp (float): Unix timestamp
        
    Returns:
        str: Formatted timestamp string
    """
    if not timestamp:
        return ""
    
    dt = datetime.fromtimestamp(timestamp)
    return dt.strftime("%Y-%m-%d %H:%M:%S")


def get_latest_version_file(files):
    """
    Get the file with the latest version from a list of files.
    
    Args:
        files (list): List of file dictionaries with 'version' key
        
    Returns:
        dict: File with the latest version, or None if list is empty
    """
    if not files:
        return None
    
    # Sort files by version (as integer)
    sorted_files = sorted(
        files,
        key=lambda x: int(x['version']) if x['version'].isdigit() else 0,
        reverse=True
    )
    
    return sorted_files[0]


def get_filetype_icon(filetype):
    """
    Get icon resource path for a file type.
    
    Args:
        filetype (str): File type (e.g., 'nk', 'aep')
        
    Returns:
        str: Resource path for icon
    """
    if filetype == 'nk':
        return ":/icons/nuke.png"
    elif filetype == 'aep':
        return ":/icons/after_effects.png"
    else:
        return ":/icons/file.png"


def parse_directory_list(directory_string):
    """
    Parse a semicolon-separated list of directories.
    
    Args:
        directory_string (str): Semicolon-separated list of directories
        
    Returns:
        list: List of directory paths
    """
    if not directory_string:
        return []
    
    # Split by semicolon and remove any empty entries
    directories = [d.strip() for d in directory_string.split(';') if d.strip()]
    
    # Validate directories
    valid_directories = []
    for directory in directories:
        if os.path.exists(directory):
            valid_directories.append(directory)
        else:
            logger.warning(f"Directory not found: {directory}")
    
    return valid_directories


def sanitize_filename(filename):
    """
    Sanitize a filename by removing invalid characters.
    
    Args:
        filename (str): Original filename
        
    Returns:
        str: Sanitized filename
    """
    # Replace invalid characters with underscore
    return re.sub(r'[\\/*?:"<>|]', '_', filename)


def get_relative_path(path, base_path):
    """
    Get the relative path from a base path.
    
    Args:
        path (str): Absolute path
        base_path (str): Base path
        
    Returns:
        str: Relative path, or original path if it's not under base_path
    """
    try:
        return os.path.relpath(path, base_path)
    except ValueError:
        return path


def human_readable_size(size_bytes):
    """
    Convert bytes to human-readable size string.
    
    Args:
        size_bytes (int): Size in bytes
        
    Returns:
        str: Human-readable size string
    """
    if size_bytes < 0:
        raise ValueError("Size must be non-negative")
    
    units = ['B', 'KB', 'MB', 'GB', 'TB']
    size = float(size_bytes)
    unit_index = 0
    
    while size >= 1024.0 and unit_index < len(units) - 1:
        size /= 1024.0
        unit_index += 1
    
    return f"{size:.2f} {units[unit_index]}"
