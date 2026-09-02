"""
User Actions Module

Purpose:
    Logs user activities such as opening files, modifying projects, etc.
    Provides a centralized way to track and record user actions.

Requirements:
    - Simple activity logging tied to username and timestamp
    - No authentication, only logging user actions
"""

import logging
import time
from datetime import datetime

# Configure logger
logger = logging.getLogger(__name__)


class UserActionLogger:
    """
    Logs user activities and actions in the application.
    """
    
    def __init__(self, db_manager=None):
        """
        Initialize the user action logger.
        
        Args:
            db_manager: Optional database manager for persisting actions
        """
        self.db_manager = db_manager
    
    def log_login(self, username):
        """
        Log user login.
        
        Args:
            username (str): Username
        """
        self._log_action(username, "login", "User logged in")
    
    def log_file_open(self, username, filepath, application):
        """
        Log file open action.
        
        Args:
            username (str): Username
            filepath (str): Path to opened file
            application (str): Application used to open file
        """
        self._log_action(
            username, 
            "file_open", 
            f"Opened file '{filepath}' with {application}"
        )
    
    def log_project_add(self, username, project_name, project_path):
        """
        Log project addition.
        
        Args:
            username (str): Username
            project_name (str): Project name
            project_path (str): Project path
        """
        self._log_action(
            username, 
            "project_add", 
            f"Added project '{project_name}' at {project_path}"
        )
    
    def log_project_remove(self, username, project_name):
        """
        Log project removal.
        
        Args:
            username (str): Username
            project_name (str): Project name
        """
        self._log_action(
            username, 
            "project_remove", 
            f"Removed project '{project_name}'"
        )
    
    def log_project_scan(self, username, directories, file_count):
        """
        Log project scan.
        
        Args:
            username (str): Username
            directories (list): List of scanned directories
            file_count (int): Number of files found
        """
        self._log_action(
            username, 
            "project_scan", 
            f"Scanned {len(directories)} directories, found {file_count} files"
        )
    
    def log_favorite_add(self, username, project_name):
        """
        Log adding a project to favorites.
        
        Args:
            username (str): Username
            project_name (str): Project name
        """
        self._log_action(
            username, 
            "favorite_add", 
            f"Added project '{project_name}' to favorites"
        )
    
    def log_favorite_remove(self, username, project_name):
        """
        Log removing a project from favorites.
        
        Args:
            username (str): Username
            project_name (str): Project name
        """
        self._log_action(
            username, 
            "favorite_remove", 
            f"Removed project '{project_name}' from favorites"
        )
    
    def log_user_add(self, username, added_username, is_admin):
        """
        Log adding a new user.
        
        Args:
            username (str): Username of admin
            added_username (str): Username of added user
            is_admin (bool): Whether the added user is an admin
        """
        admin_status = "admin" if is_admin else "regular user"
        self._log_action(
            username, 
            "user_add", 
            f"Added {admin_status} '{added_username}'"
        )
    
    def log_user_update(self, username, updated_username, is_admin):
        """
        Log updating a user.
        
        Args:
            username (str): Username of admin
            updated_username (str): Username of updated user
            is_admin (bool): New admin status
        """
        admin_status = "admin" if is_admin else "regular user"
        self._log_action(
            username, 
            "user_update", 
            f"Updated user '{updated_username}' to {admin_status}"
        )
    
    def log_user_remove(self, username, removed_username):
        """
        Log removing a user.
        
        Args:
            username (str): Username of admin
            removed_username (str): Username of removed user
        """
        self._log_action(
            username, 
            "user_remove", 
            f"Removed user '{removed_username}'"
        )
    
    def log_settings_update(self, username, setting_name, setting_value):
        """
        Log settings update.
        
        Args:
            username (str): Username
            setting_name (str): Name of updated setting
            setting_value (str): New setting value
        """
        # Mask sensitive values
        if "path" in setting_name.lower() and ":\\Program Files\\" in str(setting_value):
            masked_value = str(setting_value).split("Program Files")[0] + "Program Files\\..."
        else:
            masked_value = setting_value
            
        self._log_action(
            username, 
            "settings_update", 
            f"Updated setting '{setting_name}' to '{masked_value}'"
        )
    
    def log_custom_action(self, username, action_type, description):
        """
        Log a custom action.
        
        Args:
            username (str): Username
            action_type (str): Type of action
            description (str): Action description
        """
        self._log_action(username, action_type, description)
    
    def _log_action(self, username, action_type, description):
        """
        Log an action to the application log and the user activity table.

        Args:
            username (str): Username
            action_type (str): Type of action
            description (str): Action description
        """
        timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        log_message = f"[{timestamp}] USER: {username} | ACTION: {action_type} | {description}"

        # Log to application log
        logger.info(log_message)

        # Best-effort persistence to the database when a db_manager is available
        if self.db_manager is not None:
            try:
                self.db_manager._execute_query(
                    "INSERT INTO user_activity_log (username, action_type, description) VALUES (?, ?, ?)",
                    (username, action_type, description)
                )
            except Exception as e:
                logger.warning(f"Failed to log user action to database: {e}")
