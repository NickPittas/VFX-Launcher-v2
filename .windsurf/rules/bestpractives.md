---
trigger: always_on
---

Implementation Notes and Best Practices:
Multithreading:
Use QThreadPool and QRunnables for asynchronous tasks (e.g., scanning, launching apps, watching folders).

Logging:
Use Python's built-in logging module for easy log handling.

File Parsing Logic:
Use robust regex for extracting versions:

python
Copy
Edit
re.search(r'_v(\d{1,4})', filename)
Executable Arguments:
For Nuke: always use --nukex flag:

python
Copy
Edit
subprocess.Popen([nuke_path, '--nukex', filepath])