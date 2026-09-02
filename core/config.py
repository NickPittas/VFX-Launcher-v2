"""
Configuration Manager for VFX Launcher
Handles reading configuration from config.ini file
"""
import os
import sys
import configparser
import logging

logger = logging.getLogger(__name__)


def resolve_app_settings_path():
    """Path to app_settings.ini. Frozen builds (PyInstaller/AppImage) get a
    writable per-user copy seeded from the bundled file; dev runs use the repo file."""
    bundled = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', 'app_settings.ini'))
    if not getattr(sys, 'frozen', False):
        return bundled
    user_copy = os.path.join(os.path.expanduser('~'), '.config', 'VFX_Launcher', 'app_settings.ini')
    if not os.path.exists(user_copy):
        os.makedirs(os.path.dirname(user_copy), exist_ok=True)
        if os.path.exists(bundled):
            import shutil
            shutil.copy(bundled, user_copy)
    return user_copy

class ConfigManager:
    """Manages application configuration"""
    
    def __init__(self, config_file=None):
        """
        Initialize configuration manager
        
        Args:
            config_file: Path to config.ini file. If None, searches in standard locations.
        """
        self.config = configparser.ConfigParser()
        self.config_file = config_file or self._find_config_file()
        
        if self.config_file and os.path.exists(self.config_file):
            logger.info(f"Loading configuration from: {self.config_file}")
            self.config.read(self.config_file)
        else:
            logger.warning(f"Config file not found: {self.config_file}. Using defaults.")
            self._set_defaults()
    
    def _find_config_file(self):
        """Find config.ini in standard locations"""
        # Check in application directory (for installed version)
        app_dir = os.path.dirname(os.path.abspath(__file__))
        app_config = os.path.join(app_dir, '..', 'config.ini')
        if os.path.exists(app_config):
            return app_config
        
        # Check in executable directory (for PyInstaller)
        if getattr(sys, 'frozen', False):
            exe_dir = os.path.dirname(sys.executable)
            exe_config = os.path.join(exe_dir, 'config.ini')
            if os.path.exists(exe_config):
                return exe_config
        
        # Check in development directory
        dev_config = os.path.join(app_dir, '..', 'config.ini')
        return dev_config
    
    def _set_defaults(self):
        """Set default configuration values"""
        # Database defaults
        if not self.config.has_section('Database'):
            self.config.add_section('Database')
        
        # Default database path in user's AppData
        default_data_dir = os.path.join(
            os.environ.get('APPDATA', os.path.expanduser('~')),
            'VFX_Launcher'
        )
        
        if not self.config.has_option('Database', 'Path'):
            self.config.set('Database', 'Path', 
                          os.path.join(default_data_dir, 'vfx_launcher.db'))
        
        # Paths defaults
        if not self.config.has_section('Paths'):
            self.config.add_section('Paths')
        
        if not self.config.has_option('Paths', 'DataDir'):
            self.config.set('Paths', 'DataDir', default_data_dir)
    
    def get_database_path(self):
        """Get the database file path"""
        db_path = self.config.get('Database', 'Path', 
                                  fallback=os.path.join(os.path.dirname(__file__), '..', 'vfx_launcher.db'))
        
        # Ensure the directory exists
        db_dir = os.path.dirname(db_path)
        if not os.path.exists(db_dir):
            logger.info(f"Creating database directory: {db_dir}")
            os.makedirs(db_dir, exist_ok=True)
        
        return db_path
    
    def get_data_dir(self):
        """Get the data directory path"""
        data_dir = self.config.get('Paths', 'DataDir',
                                   fallback=os.path.join(os.path.dirname(__file__), '..'))
        
        # Ensure the directory exists
        if not os.path.exists(data_dir):
            logger.info(f"Creating data directory: {data_dir}")
            os.makedirs(data_dir, exist_ok=True)
        
        return data_dir
    
    def get_config_dir(self):
        """Get the config directory path"""
        data_dir = self.get_data_dir()
        config_dir = os.path.join(data_dir, 'config')
        
        # Ensure the directory exists
        if not os.path.exists(config_dir):
            logger.info(f"Creating config directory: {config_dir}")
            os.makedirs(config_dir, exist_ok=True)
        
        return config_dir
    
    def save(self):
        """Save configuration to file"""
        if self.config_file:
            with open(self.config_file, 'w') as f:
                self.config.write(f)
            logger.info(f"Configuration saved to: {self.config_file}")

