"""Execute 09_openrouter.ipynb against the live API and report cell output."""

import nbformat
from nbclient import NotebookClient

nb = nbformat.read("examples/python-notebooks/09_openrouter.ipynb", as_version=4)

client = NotebookClient(
    nb,
    timeout=1200,
    kernel_name="python3",
    resources={"metadata": {"path": "examples/python-notebooks"}},
    allow_errors=True,
)
client.execute()

for index, cell in enumerate(nb.cells):
    if cell.cell_type != "code":
        continue
    for output in cell.get("outputs", []):
        if output.output_type == "stream":
            print(f"[{index}] {output.text.rstrip()}")
        elif output.output_type == "error":
            print(f"[{index}] ERROR {output.ename}: {output.evalue}")
