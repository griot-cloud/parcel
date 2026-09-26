"""Sphinx configuration for the parcel documentation site."""

from datetime import date

project = "parcel"
copyright = f"{date.today().year}, Griot Data Technologies"
author = "Griot Data Technologies"
release = "0.0.1"

extensions = ["myst_parser", "sphinx_design"]
source_suffix = {".md": "markdown"}
exclude_patterns = ["_build"]

myst_enable_extensions = ["colon_fence", "deflist", "fieldlist"]
myst_heading_anchors = 3

html_theme = "shibuya"
html_title = "parcel — Data contracts"
html_static_path = ["_static"]
html_css_files = ["parcel.css"]
html_theme_options = {
    "github_url": "https://github.com/griot-cloud/parcel",
    "nav_links": [
        {"title": "peQL", "url": "https://griot-cloud.github.io/peQL/", "external": True},
        {"title": "GitHub", "url": "https://github.com/griot-cloud/parcel", "external": True},
    ],
}
