# The introduction page

The page published at <https://uthree.github.io/auris-studio/> by
`.github/workflows/pages.yml`. It is plain HTML, CSS and an ES module with no build step, drawn
with the [Atelier](https://github.com/uthree/design-language) web kit so that it follows the
application's Glass themes.

`tools/site/build.sh` assembles it: this directory, the README screenshots from
`docs/screenshots/readme/`, the application icon from `crates/auris-gpui/assets/`, and the
Atelier kit fetched at the revision the script pins. To look at it locally:

```sh
tools/site/build.sh
python3 -m http.server --directory target/site
```

Then open <http://localhost:8000/>. `?lang=ja` or `?lang=en` picks the language; otherwise it
follows the browser and remembers the switch. Every sentence is written twice, once under
`data-l="en"` and once under `data-l="ja"`, and only the selected one is shown. A change to the
README's description of the application should be carried here too.
