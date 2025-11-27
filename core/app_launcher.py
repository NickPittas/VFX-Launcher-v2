"""
Application Launching Module

Purpose:
    Opens selected .nk (with --nukex) or .aep files in respective software applications.
    Handles subprocess calls to launch external applications.

Requirements:
    - Subprocess calls with proper parameters
    - Path verification and logging
    - Error handling for missing executables or files
"""

import os
import subprocess
import logging
import platform
from pathlib import Path

# Configure logger
logger = logging.getLogger(__name__)

class AppLauncher:
    """
    Handles launching external VFX applications with the appropriate parameters.
    """
    
    def __init__(self, nuke_path=None, after_effects_path=None):
        """
        Initialize the application launcher with paths to the executables.
        
        Args:
            nuke_path (str, optional): Path to Nuke executable
            after_effects_path (str, optional): Path to After Effects executable
        """
        self.nuke_path = nuke_path
        self.after_effects_path = after_effects_path
    
    def set_nuke_path(self, path):
        """
        Set the path to the Nuke executable.
        
        Args:
            path (str): Path to Nuke executable
            
        Returns:
            bool: True if path exists and is valid
        """
        if not path or not os.path.exists(path):
            logger.warning(f"Invalid Nuke path: {path}")
            return False
        
        self.nuke_path = path
        logger.info(f"Nuke path set to: {path}")
        return True
    
    def set_after_effects_path(self, path):
        """
        Set the path to the After Effects executable.
        
        Args:
            path (str): Path to After Effects executable
            
        Returns:
            bool: True if path exists and is valid
        """
        if not path or not os.path.exists(path):
            logger.warning(f"Invalid After Effects path: {path}")
            return False
        
        self.after_effects_path = path
        logger.info(f"After Effects path set to: {path}")
        return True
    
    def launch_nuke(self, filepath, user=None):
        """
        Launch Nuke with the specified file.
        
        Args:
            filepath (str): Path to .nk file
            user (str, optional): Username for logging
            
        Returns:
            bool: True if launch successful
        """
        if not self.nuke_path:
            logger.error("Nuke path not set")
            return False
        
        if not os.path.exists(filepath):
            logger.error(f"File not found: {filepath}")
            return False
        
        try:
            # On Windows, use native backslashes for paths (do not convert to POSIX)
            # Prepare command with --nukex flag for Nuke X
            cmd = [self.nuke_path, "--nukex", filepath]
            
            # Log the action
            logger.info(f"Launching Nuke with file: {filepath}")
            if user:
                logger.info(f"User: {user}")
            
            # Set working directory to file's folder
            cwd = os.path.dirname(filepath)
            # Log the full command and working directory
            logger.info(f"Nuke launch command: {cmd}")
            logger.info(f"Nuke launch cwd: {cwd}")
            # Launch Nuke in a new process without waiting, capture output
            proc = subprocess.Popen(cmd, cwd=cwd, shell=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            # Give the process a moment to fail if it will
            import time
            time.sleep(1)
            if proc.poll() is not None:
                out, err = proc.communicate()
                logger.error(f"Nuke launch failed. stdout: {out.decode(errors='ignore')}, stderr: {err.decode(errors='ignore')}")
                return False
            return True
        except Exception as e:
            logger.error(f"Error launching Nuke: {str(e)}")
            return False
    
    def launch_after_effects(self, filepath, user=None):
        """
        Launch After Effects with the specified file.
        
        Args:
            filepath (str): Path to .aep file
            user (str, optional): Username for logging
            
        Returns:
            bool: True if launch successful
        """
        if not self.after_effects_path:
            logger.error("After Effects path not set")
            return False
        
        if not os.path.exists(filepath):
            logger.error(f"File not found: {filepath}")
            return False
        
        try:
            # Normalize path to work with subprocess
            filepath = Path(filepath).as_posix()
            
            # Prepare command
            cmd = [self.after_effects_path, filepath]
            
            # Log the action
            logger.info(f"Launching After Effects with file: {filepath}")
            if user:
                logger.info(f"User: {user}")
            
            # Launch After Effects in a new process without waiting
            subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            
            return True
        
        except Exception as e:
            logger.error(f"Error launching After Effects: {str(e)}")
            return False
    
    def launch_file(self, filepath, user=None):
        """
        Launch the appropriate application based on file extension.
        
        Args:
            filepath (str): Path to file
            user (str, optional): Username for logging
            
        Returns:
            bool: True if launch successful
        """
        if not os.path.exists(filepath):
            logger.error(f"File not found: {filepath}")
            return False
        
        # Determine file type from extension
        _, ext = os.path.splitext(filepath)
        ext = ext.lower()
        
        if ext == '.nk':
            return self.launch_nuke(filepath, user)
        elif ext == '.aep':
            return self.launch_after_effects(filepath, user)
        else:
            logger.error(f"Unsupported file type: {ext}")
            return False
    
    def open_containing_folder(self, filepath):
        """
        Open the folder containing the file in the system file explorer.
        
        Args:
            filepath (str): Path to file
            
        Returns:
            bool: True if successful
        """
        if not os.path.exists(filepath):
            filepath = os.path.dirname(filepath)
            if not os.path.exists(filepath):
                logger.error(f"Path not found: {filepath}")
                return False
        
        try:
            folder_path = os.path.dirname(filepath)
            
            # Open folder with system file explorer
            if platform.system() == "Windows":
                os.startfile(folder_path)
            elif platform.system() == "Darwin":  # macOS
                subprocess.Popen(["open", folder_path])
            else:  # Linux
                subprocess.Popen(["xdg-open", folder_path])
            
            logger.info(f"Opened folder: {folder_path}")
            return True
            
        except Exception as e:
            logger.error(f"Error opening folder: {str(e)}")
            return False
