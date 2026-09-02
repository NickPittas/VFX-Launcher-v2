"""
Tree Manager Module - Handles tree widget operations for file browser
"""
import os
import logging
from PySide6.QtWidgets import QTreeWidgetItem, QWidget, QLabel, QHBoxLayout, QComboBox
from PySide6.QtGui import QIcon, QColor, QBrush, QPixmap
from PySide6.QtCore import QSize, Qt

from .file_widgets import VersionedFileWidget
from .file_handlers import FileHandler

logger = logging.getLogger(__name__)

class FileTreeManager:
    """Manages file tree widgets and their contents"""
    
    def __init__(self, tree_widget):
        """Initialize with a tree widget"""
        self.tree_widget = tree_widget
        self.icon_cache = {}  # Cache icons to avoid repeated loading
    
    def clear(self):
        """Clear the tree widget"""
        self.tree_widget.clear()
    
    def get_file_icon(self, file_type):
        """Get icon for file type (cached)"""
        if file_type not in self.icon_cache:
            # Get the icons directory path
            icons_dir = os.path.join(os.path.dirname(__file__), "icons")

            # Icon paths based on file type
            icon_filename = {
                'nk': "nuke.svg",
                'aep': "after-effects.svg"
            }.get(file_type.lower(), "file_icon.png")

            icon_path = os.path.join(icons_dir, icon_filename)

            # Load icon if file exists, otherwise use empty icon
            if os.path.exists(icon_path):
                self.icon_cache[file_type] = QIcon(icon_path)
                logger.info(f"Loaded file icon for {file_type}: {icon_path}")
            else:
                logger.warning(f"Icon not found: {icon_path}")
                self.icon_cache[file_type] = QIcon()

        return self.icon_cache[file_type]
    
    def add_shot_item(self, shot_name, parent_item=None):
        """Add a shot/folder item to the tree with custom widget for text clarity"""
        # Create empty item first
        shot_item = QTreeWidgetItem()
        shot_item.setText(0, "")  # Explicitly empty text
        shot_item.setForeground(0, QBrush(QColor(0, 0, 0, 0)))  # Transparent text
        if parent_item is not None:
            parent_item.addChild(shot_item)
        else:
            self.tree_widget.addTopLevelItem(shot_item)
        
        # Create custom widget for text display
        shot_widget = QWidget()
        layout = QHBoxLayout(shot_widget)
        layout.setContentsMargins(2, 2, 2, 2)
        layout.setSpacing(5)
        
        # Add folder icon
        icon_label = QLabel()
        folder_icon_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "icons", "folder_icon.png")
        icon_label.setPixmap(QIcon(folder_icon_path).pixmap(16, 16))
        layout.addWidget(icon_label)
        
        # Add shot name with spacing for clarity
        text_label = QLabel()
        text_label.setText(shot_name)
        text_label.setTextFormat(Qt.PlainText)
        text_label.setAlignment(Qt.AlignLeft | Qt.AlignVCenter)
        text_label.setStyleSheet("""color: #f0f0f0; font-weight: bold; font-size: 13px;""")
        layout.addWidget(text_label)
        
        # Push everything to the left
        layout.addStretch(1)
        
        # Set the custom widget
        self.tree_widget.setItemWidget(shot_item, 0, shot_widget)
        
        return shot_item
    
    def add_single_file(self, file_data, parent_item=None):
        """Add a single file item to the tree with custom widget to avoid text issues"""
        try:
            filename = file_data.get('filename', 'Unknown')
            filepath = file_data.get('filepath', '')
            filetype = file_data.get('filetype', '').lower()
            last_opened = file_data.get('last_opened', '')
            opened_by = file_data.get('opened_by', '')
            file_id = file_data.get('id', None)

            # Create a completely empty tree item - no text at all
            # The emptiness is crucial to avoid text rendering issues
            file_item = QTreeWidgetItem()
            file_item.setText(0, "")
            file_item.setToolTip(0, filepath)
            file_item.setData(0, 256, filepath)  # Store filepath for launching
            file_item.setData(0, 257, file_id)  # Store file ID for access logging

            # Set Last Opened and Opened By columns
            if last_opened:
                # Format timestamp to readable date/time
                from datetime import datetime
                try:
                    dt = datetime.fromisoformat(last_opened.replace('Z', '+00:00'))
                    formatted_time = dt.strftime('%Y-%m-%d %H:%M')
                except:
                    formatted_time = last_opened
                file_item.setText(1, formatted_time)
            else:
                file_item.setText(1, "Never")

            file_item.setText(2, opened_by if opened_by else "-")

            # Hide any potential text by setting transparent foreground for column 0 only
            file_item.setForeground(0, QBrush(QColor(0, 0, 0, 0)))

            # Calculate proper height for the tree item based on content
            file_item.setSizeHint(0, QSize(400, 26))
            
            # Add the item to the tree in the right place
            if parent_item:
                parent_item.addChild(file_item)
            else:
                self.tree_widget.addTopLevelItem(file_item)
            
            # Create a custom widget for proper text display
            file_widget = QWidget()
            layout = QHBoxLayout(file_widget)
            layout.setContentsMargins(2, 2, 2, 2)
            layout.setSpacing(5)
            
            # Add file icon
            icon_label = QLabel()
            icon_label.setPixmap(self.get_file_icon(filetype).pixmap(16, 16))
            layout.addWidget(icon_label)
            
            # Display filename normally without inserting spaces between characters
            text_label = QLabel(filename)
            text_label.setTextFormat(Qt.PlainText)  # Crucial for proper text display
            text_label.setAlignment(Qt.AlignLeft | Qt.AlignVCenter)
            text_label.setStyleSheet("""
                color: #f0f0f0; 
                font-family: 'Segoe UI', Arial, sans-serif; 
                font-size: 12px;
                letter-spacing: normal;
                word-spacing: normal;
            """)
            layout.addWidget(text_label)
            
            # Add spacer to push everything to the left
            layout.addStretch(1)
            
            # Set the custom widget on the tree item
            self.tree_widget.setItemWidget(file_item, 0, file_widget)
            
            return file_item
        except Exception as e:
            logger.error(f"[TreeManager] Error adding file {file_data.get('filename', 'Unknown')}: {str(e)}")
            return None
    
    def handle_versioned_files(self, file_versions, shot_item, base_name, file_type):
        """Handle versioned files in the tree"""
        try:
            # Sort versions and get version numbers
            versions_sorted, version_numbers = FileHandler.sort_by_version(file_versions)
            file_icon = self.get_file_icon(file_type)
            
            # Versioned files: show one node with dropdown
            if len(file_versions) > 1:
                # Create an EMPTY tree item - crucial to avoid text rendering issues
                file_item = QTreeWidgetItem()
                file_item.setText(0, "")  # Explicitly empty text
                file_item.setToolTip(0, f"File: {base_name}\nVersions: {', '.join(version_numbers)}")
                file_item.setForeground(0, QBrush(QColor(0, 0, 0, 0)))  # Transparent foreground
                shot_item.addChild(file_item)
                
                # Create a mapping of version numbers to file paths for quick lookup
                file_paths_by_version = {}
                for idx, file in enumerate(versions_sorted):
                    if idx < len(version_numbers):
                        version = version_numbers[idx]
                        file_paths_by_version[version] = file.get('filepath', '')
                
                # Custom version change handler (connected once inside create_file_widget)
                def on_version_change(idx):
                    if idx >= 0 and idx < len(version_numbers):
                        selected_version = version_numbers[idx]
                        selected_path = file_paths_by_version.get(selected_version, '')
                        selected_file = versions_sorted[idx] if idx < len(versions_sorted) else None

                        # Update filepath for launching
                        file_item.setData(0, 256, selected_path)

                        # Update access info for selected version
                        if selected_file:
                            file_id = selected_file.get('id', None)
                            file_item.setData(0, 257, file_id)  # Update file ID

                            last_opened = selected_file.get('last_opened', '')
                            opened_by = selected_file.get('opened_by', '')

                            if last_opened:
                                from datetime import datetime
                                try:
                                    dt = datetime.fromisoformat(last_opened.replace('Z', '+00:00'))
                                    formatted_time = dt.strftime('%Y-%m-%d %H:%M')
                                except:
                                    formatted_time = last_opened
                                file_item.setText(1, formatted_time)
                            else:
                                file_item.setText(1, "Never")

                            file_item.setText(2, opened_by if opened_by else "-")

                        logger.debug(f"Updated file path to {selected_path} for version {selected_version}")
                file_widget = VersionedFileWidget.create_file_widget(
                    base_name,
                    version_numbers,
                    file_item,
                    self.tree_widget,
                    file_icon,  # Pass the file icon
                    on_version_change  # Unique path/access-info logic; badge handled in file_widgets
                )
                
                # Find the combo box in the widget
                combo = None
                for child in file_widget.findChildren(QComboBox):
                    combo = child
                    break
                
                # Find the version badge (QLabel)
                version_badge = None
                for child in file_widget.findChildren(QLabel):
                    if child.text() in version_numbers:
                        version_badge = child
                        break
                
                # Set the highest version as the default selected path
                highest_version = version_numbers[-1] if version_numbers else "1"
                highest_path = file_paths_by_version.get(highest_version, '')
                file_item.setData(0, 256, highest_path)  # Store path for launching

                # Get the highest version file data for access info
                highest_file = versions_sorted[-1] if versions_sorted else None
                if highest_file:
                    file_id = highest_file.get('id', None)
                    file_item.setData(0, 257, file_id)  # Store file ID

                    # Set Last Opened and Opened By columns for highest version
                    last_opened = highest_file.get('last_opened', '')
                    opened_by = highest_file.get('opened_by', '')

                    if last_opened:
                        from datetime import datetime
                        try:
                            dt = datetime.fromisoformat(last_opened.replace('Z', '+00:00'))
                            formatted_time = dt.strftime('%Y-%m-%d %H:%M')
                        except:
                            formatted_time = last_opened
                        file_item.setText(1, formatted_time)
                    else:
                        file_item.setText(1, "Never")

                    file_item.setText(2, opened_by if opened_by else "-")

            else:
                # Single version - just add as a regular file
                single_file = file_versions[0] if file_versions else None
                if single_file:
                    self.add_single_file(single_file, shot_item)
                    
        except Exception as e:
            logger.error(f"[TreeManager] Error handling versioned files for {base_name}: {str(e)}")
            # Fall back to simple tree items
            for file in file_versions:
                self.add_single_file(file, shot_item)
    
    def populate_tree(self, files, group_by_shot=True, handle_versions=True):
        """Populate the tree with files, handling shots and versions"""
        try:
            if not files:
                return
            
            self.clear()
            
            # Prepare tree widget for better readability and proper tree behavior
            self.tree_widget.setIndentation(20)  # Increase indentation for better hierarchy
            self.tree_widget.setUniformRowHeights(False)  # Allow different row heights for custom widgets
            self.tree_widget.setAnimated(True)  # Smooth animations
            self.tree_widget.setWordWrap(False)  # Prevent word wrapping for cleaner display
            
            # Ensure expand/contract arrows are visible
            self.tree_widget.setRootIsDecorated(True)  # Show decorations (expand arrows)
            self.tree_widget.setItemsExpandable(True)  # Allow items to be expanded/collapsed
            
            # Use standard system arrows that will work on all platforms
            self.tree_widget.setStyleSheet("""
                QTreeWidget {
                    background-color: #2a2a2a;
                    color: #f0f0f0;
                    border: 1px solid #444444;
                    border-radius: 4px;
                    alternate-background-color: #323232;
                }
                
                QTreeWidget::item {
                    padding: 5px 3px;
                    border-bottom: 1px solid #333333;
                }
                
                QTreeWidget::item:selected {
                    background-color: #3d3d3d;
                    color: #ffffff;
                }
            """)
            
            # No need to apply additional styles - they're already set above
            
            if group_by_shot:
                # Group files by timeline, then shot (Projects/Nuke/<timeline>/<shot>/file)
                timeline_groups = FileHandler.group_by_timeline(files)

                for timeline_name in sorted(timeline_groups.keys()):
                    # Empty timeline = not under a timeline folder; shots go top-level
                    timeline_item = self.add_shot_item(timeline_name) if timeline_name else None

                    for shot_name in sorted(timeline_groups[timeline_name].keys()):
                        shot_files = timeline_groups[timeline_name][shot_name]
                        shot_item = self.add_shot_item(shot_name, parent_item=timeline_item)

                        if handle_versions:
                            # Group files by base name
                            versioned_groups = FileHandler.group_versioned_files(shot_files)

                            # Sort base names alphabetically
                            base_names = sorted(versioned_groups.keys())

                            # Process each group of versions in sorted order
                            for base_name in base_names:
                                file_versions = versioned_groups[base_name]
                                file_type = file_versions[0].get('filetype', '') if file_versions else ''
                                self.handle_versioned_files(file_versions, shot_item, base_name, file_type)
                        else:
                            # Sort files by filename
                            sorted_files = sorted(shot_files, key=lambda f: f.get('filename', ''))

                            # Add files directly without version handling
                            for file in sorted_files:
                                self.add_single_file(file, shot_item)
            else:
                # Sort files by filename
                sorted_files = sorted(files, key=lambda f: f.get('filename', ''))
                
                # Add files directly at the top level
                for file in sorted_files:
                    self.add_single_file(file)
                    
            # Keep all groups collapsed by default for a cleaner initial view
            # User can expand groups as needed to see the files
            for i in range(self.tree_widget.topLevelItemCount()):
                top_item = self.tree_widget.topLevelItem(i)
                top_item.setExpanded(False)  # Keep all groups collapsed by default
            
            # Resize columns to content
            self.tree_widget.resizeColumnToContents(0)
            
        except Exception as e:
            logger.error(f"[TreeManager] Error populating tree: {str(e)}")
