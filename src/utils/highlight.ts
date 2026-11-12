// Code highlighting: the `common` build (37 bundled grammars) plus the extra
// grammars LLMs routinely emit. Importing the package root pulls all ~190
// grammars into the main chunk (~0.9 MB), so nothing here may switch back to
// `highlight.js` directly.
import hljs from "highlight.js/lib/common";
import clojure from "highlight.js/lib/languages/clojure";
import cmake from "highlight.js/lib/languages/cmake";
import dart from "highlight.js/lib/languages/dart";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import elixir from "highlight.js/lib/languages/elixir";
import groovy from "highlight.js/lib/languages/groovy";
import haskell from "highlight.js/lib/languages/haskell";
import http from "highlight.js/lib/languages/http";
import nginx from "highlight.js/lib/languages/nginx";
import powershell from "highlight.js/lib/languages/powershell";
import protobuf from "highlight.js/lib/languages/protobuf";
import scala from "highlight.js/lib/languages/scala";

const extraLanguages = {
  clojure,
  cmake,
  dart,
  dockerfile,
  elixir,
  groovy,
  haskell,
  http,
  nginx,
  powershell,
  protobuf,
  scala,
};

for (const [name, language] of Object.entries(extraLanguages)) {
  hljs.registerLanguage(name, language);
}

export default hljs;
