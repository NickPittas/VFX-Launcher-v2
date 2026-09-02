"""
File Handlers Module - Logic for processing different file types
"""
import os
import re
import logging
from PySide6.QtWidgets import QTreeWidgetItem
from PySide6.QtGui import QIcon

logger = logging.getLogger(__name__)

class FileHandler:
    """Base class for file handlers"""
    
    @staticmethod
    def extract_version(filename):
        """
        Extract version number from filename using regex.
        For AEP files: if no _v pattern found, use the last number in the filename (1-5 digits).
        For other files: only use _v pattern.
        """
        try:
            # First try the standard _v pattern (works for all file types)
            match = re.search(r'_v(\d{1,4})', filename)
            if match:
                return match.group(1)

            # For AEP files only, try to extract the last number in the filename
            if filename.lower().endswith('.aep'):
                # Remove the extension first
                name_without_ext = os.path.splitext(filename)[0]
                # Find the last number (1-5 digits) in the filename
                last_number_match = re.search(r'(\d{1,5})(?!.*\d)', name_without_ext)
                if last_number_match:
                    return last_number_match.group(1)

            return "1"  # Default version if not found
        except Exception as e:
            logger.error(f"Error extracting version from {filename}: {str(e)}")
            return "1"
    
    @staticmethod
    def get_base_name(filename):
        """
        Get base name without version from filename.
        For AEP files: removes the last number (1-5 digits) if no _v pattern exists.
        For other files: only removes _v pattern.
        """
        try:
            # First try to remove the standard _v pattern
            base_name = re.sub(r'_v\d{1,4}', '', filename)

            # If the filename changed (had _v pattern), return it
            if base_name != filename:
                return base_name

            # For AEP files only, try to remove the last number
            if filename.lower().endswith('.aep'):
                # Remove the extension first
                name_without_ext = os.path.splitext(filename)[0]
                ext = os.path.splitext(filename)[1]

                # Remove the last number (1-5 digits) from the filename
                base_without_version = re.sub(r'(\d{1,5})(?!.*\d)$', '', name_without_ext)

                # If something was removed, return the base name with extension
                if base_without_version != name_without_ext:
                    # Also remove trailing underscore or dash if present
                    base_without_version = base_without_version.rstrip('_-')
                    return base_without_version + ext

            return filename
        except Exception as e:
            logger.error(f"Error getting base name from {filename}: {str(e)}")
            return filename
    
    @staticmethod
    def group_by_shot(files):
        """Group files by shot/sequence name"""
        shot_groups = {}
        try:
            for file in files:
                filepath = file.get('filepath', '')
                filename = file.get('filename', '')
                
                # Try to extract shot name from filepath or filename
                # First check if there's a folder structure with shot name
                folder_path = os.path.dirname(filepath)
                shot_name = os.path.basename(folder_path) if folder_path else ""
                
                # If no folder name found, try to extract from filename using common patterns
                if not shot_name or shot_name.lower() in ['nk', 'aep', 'shots', 'assets', 'nuke', 'after effects', 'script', 'scripts']:
                    # Try to extract shot name from filename (before version number)
                    name_parts = filename.split('_v') if '_v' in filename else [filename]
                    base_name = name_parts[0]
                    
                    # Check for shot code patterns (e.g., SH010, sh_020)
                    shot_match = re.search(r'((?:SH|sh)[_-]?\d{2,3})', base_name)
                    if shot_match:
                        shot_name = shot_match.group(1)
                    else:
                        # Use first part of filename as shot name
                        shot_parts = base_name.split('_')
                        shot_name = shot_parts[0] if shot_parts else "Main"
                
                # Default to "Main" if no shot name found
                if not shot_name:
                    shot_name = "Main"
                
                # Create group if it doesn't exist
                if shot_name not in shot_groups:
                    shot_groups[shot_name] = []
                
                shot_groups[shot_name].append(file)
        except Exception as e:
            logger.error(f"Error grouping files by shot: {str(e)}")
            # Fall back to simple grouping
            for file in files:
                shot_name = "Main"
                if shot_name not in shot_groups:
                    shot_groups[shot_name] = []
                shot_groups[shot_name].append(file)
        
        return shot_groups

    @staticmethod
    def group_by_timeline(files):
        """
        Group files into {timeline: {shot: [files]}}.
        Levels are derived relative to the Projects/Nuke (or After Effects) anchor:
        <anchor>/<timeline>/<shot>/.../file.nk — extra nesting (artist folders such
        as <shot>/nuke/script) below the shot is ignored. The TOPMOST anchor that
        still yields a timeline+shot pair wins, so a per-shot 'nuke' subfolder can
        never be mistaken for the project-level Nuke root. Files not at least two
        folders below any anchor land in the '' timeline group using the
        shot-name fallback heuristics.
        """
        timeline_groups = {}
        for f in files:
            parts = [p for p in re.split(r'[\\/]', f.get('filepath', '')) if p]
            anchors = [i for i, p in enumerate(parts[:-1])
                       if p.lower() in ('nuke', 'after effects', 'aftereffects')]
            timeline = shot = None
            # Prefer the topmost anchor with a full timeline+shot pair; fall back
            # to the topmost anchor with at least a shot segment.
            for needed in (2, 1):
                for anchor in anchors:
                    rel = parts[anchor + 1:-1]
                    if len(rel) >= needed:
                        timeline = rel[0] if len(rel) >= 2 else ''
                        shot = rel[1] if len(rel) >= 2 else rel[0]
                        break
                if shot is not None:
                    break
            if shot is None:
                timeline = ''
                shot = next(iter(FileHandler.group_by_shot([f])))  # filename heuristic
            timeline_groups.setdefault(timeline, {}).setdefault(shot, []).append(f)
        return timeline_groups

    @staticmethod
    def group_versioned_files(files):
        """Group files by base name and collect versions"""
        versioned_groups = {}
        try:
            for file in files:
                filename = file.get('filename', '')
                base_name = FileHandler.get_base_name(filename)
                
                if base_name not in versioned_groups:
                    versioned_groups[base_name] = []
                
                versioned_groups[base_name].append(file)
        except Exception as e:
            logger.error(f"Error grouping versioned files: {str(e)}")
        
        return versioned_groups
    
    @staticmethod
    def sort_by_version(files):
        """Sort files by version number and remove duplicates"""
        try:
            # Deduplicate files by filepath first
            seen_paths = set()
            unique_files = []
            for file in files:
                filepath = file.get('filepath', '')
                if filepath and filepath not in seen_paths:
                    seen_paths.add(filepath)
                    unique_files.append(file)

            # Extract version numbers from unique files
            version_numbers = [FileHandler.extract_version(f.get('filename', '')) for f in unique_files]

            # Create pairs of (version, file) for sorting
            version_file_pairs = zip(version_numbers, unique_files)

            # Sort by version number (as integer)
            sorted_pairs = sorted(version_file_pairs, key=lambda x: int(x[0]))

            # Unzip back to separate lists
            version_numbers_sorted, files_sorted = zip(*sorted_pairs) if sorted_pairs else ([], [])

            return list(files_sorted), list(version_numbers_sorted)
        except Exception as e:
            logger.error(f"Error sorting files by version: {str(e)}")
            return files, [FileHandler.extract_version(f.get('filename', '')) for f in files]


if __name__ == "__main__":
    # Self-check: timeline grouping over Projects/Nuke/<timeline>/<shot>/file.nk
    files = [
        {'filename': 'shotA_comp_v01.nk', 'filepath': '/proj/Projects/Nuke/Timeline1/shotA/shotA_comp_v01.nk'},
        {'filename': 'shotB_comp_v02.nk', 'filepath': '/proj/Projects/Nuke/Timeline1/shotB/shotB_comp_v02.nk'},
        {'filename': 'shotC_comp_v01.nk', 'filepath': '/proj/Projects/Nuke/Timeline2/shotC/shotC_comp_v01.nk'},
        {'filename': 'loose_comp_v01.nk', 'filepath': '/proj/Projects/Nuke/loose_comp_v01.nk'},
        # Deep nesting: artist folder below the shot is ignored
        {'filename': 'x_comp_v02.nk', 'filepath': '/proj/Projects/Nuke/080126_Edit_Novibet_pre 04_93sec/A_0004C017_260725_113917_a1DX4/Paul/x_comp_v02.nk'},
        # Windows-style separators from the DB
        {'filename': 'y_comp_v01.nk', 'filepath': 'H:\\Projects\\Nuke\\Timeline1\\shotA\\y_comp_v01.nk'},
    ]
    groups = FileHandler.group_by_timeline(files)
    assert set(groups) == {'Timeline1', 'Timeline2', '080126_Edit_Novibet_pre 04_93sec', ''}, groups.keys()
    assert set(groups['Timeline1']) == {'shotA', 'shotB'}
    assert len(groups['Timeline1']['shotA']) == 2  # linux + windows path in same group
    assert set(groups['Timeline2']) == {'shotC'}
    assert set(groups['080126_Edit_Novibet_pre 04_93sec']) == {'A_0004C017_260725_113917_a1DX4'}
    assert set(groups['']) == {'loose'}  # loose file: shot name from filename, no timeline
    print("self-check OK")
