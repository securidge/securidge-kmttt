// 공개 문서 점검: GitHub에서 깨져 보이거나 비어 보이는 곳을 찾는다.
//   - 빈 절: 제목 바로 다음이 같은 단계 이상의 제목이거나 파일 끝
//   - 상대 링크: 파일이 없거나, #앵커가 대상 문서의 제목에 없음(GitHub 앵커 규칙)
//   - 실수 링크: "[버튼](Alt+Y)", "[항목](아이콘)"처럼 괄호를 붙여 써서 링크가 된 것
//   - HTML <img src>, <a href>, <source src>와 이 저장소를 가리키는 절대 주소
//     (github.com/…/blob|tree/main/…, raw.githubusercontent.com/…/main/…)도 같은 기준으로 확인
//   - 한·영 짝 문서: 제목 개수와 단계가 같은지
// 사용: node ci/check-docs.js [저장소 루트]
const fs = require("fs");
const path = require("path");
const root = path.resolve(process.argv[2] || ".");

const DOCS = [
  "README.md", "README.ko.md", "SECURITY.md", "CHANGELOG.md",
  "docs/USER_GUIDE.md", "docs/USER_GUIDE.ko.md", "docs/DESIGN.md", "docs/DECISIONS.md", "docs/SECURITY-REVIEW.md",
  ...fs.readdirSync(path.join(root, "docs/releases")).filter((f) => f.endsWith(".md")).map((f) => "docs/releases/" + f),
];
const PAIRS = [["README.md", "README.ko.md"], ["docs/USER_GUIDE.md", "docs/USER_GUIDE.ko.md"]];

// 이 저장소의 절대 주소 → 저장소 안 경로(와 앵커).
const SELF = /^https:\/\/(?:github\.com\/securidge\/securidge-kmttt\/(?:blob|tree)\/main|raw\.githubusercontent\.com\/securidge\/securidge-kmttt\/main)\/([^?#]*)(?:#(.*))?$/;

// GitHub 제목 앵커: 소문자, 글자·숫자·공백·하이픈·밑줄만 남기고 공백을 하이픈으로.
const slug = (h) => h.trim().toLowerCase().replace(/[^\p{L}\p{N}\p{M} _-]/gu, "").replace(/ /g, "-");

const cache = new Map();
function parse(rel) {
  rel = rel.replace(/\\/g, "/");
  if (cache.has(rel)) return cache.get(rel);
  const lines = fs.readFileSync(path.join(root, rel), "utf8").split(/\r?\n/);
  const heads = [];
  let fence = false;
  lines.forEach((l, i) => {
    if (/^\s*```/.test(l)) fence = !fence;
    if (fence) return;
    const m = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(l);
    if (m) heads.push({ line: i, level: m[1].length, text: m[2] });
  });
  const anchors = new Set();
  const seen = new Map();
  for (const h of heads) {
    const s = slug(h.text);
    const n = seen.get(s) || 0;
    anchors.add(n ? `${s}-${n}` : s);
    seen.set(s, n + 1);
  }
  const doc = { lines, heads, anchors };
  cache.set(rel, doc);
  return doc;
}

const problems = [];
const report = (rel, line, msg) => problems.push(`${rel}:${line + 1}: ${msg}`);

// rel 문서 line에 있는 링크 url을 확인한다. fromRoot면 경로가 저장소 루트 기준.
function checkTarget(rel, line, url, fromRoot) {
  const [p, frag] = url.split("#");
  const hasAnchor = (doc) => parse(doc).anchors.has(decodeURIComponent(frag).toLowerCase());
  if (p) {
    const target = path.normalize(fromRoot ? p : path.join(path.dirname(rel), p));
    if (!fs.existsSync(path.join(root, target))) return report(rel, line, `broken link ${url}`);
    if (frag && target.endsWith(".md") && !hasAnchor(target)) report(rel, line, `broken anchor ${url}`);
  } else if (frag && !hasAnchor(rel)) {
    report(rel, line, `broken anchor #${frag}`);
  }
}

// 절대 주소면 이 저장소 주소인지 보고 확인한다. 처리했으면 true.
function checkAbsolute(rel, line, url) {
  const self = SELF.exec(url.replace(/[.,;:]+$/, ""));
  if (self) checkTarget(rel, line, self[1] + (self[2] ? "#" + self[2] : ""), true);
  return /^(https?:|mailto:|data:)/.test(url);
}

for (const rel of DOCS) {
  if (!fs.existsSync(path.join(root, rel))) { problems.push(`${rel}: missing`); continue; }
  const { lines, heads } = parse(rel);

  heads.forEach((h, k) => {
    let j = h.line + 1;
    while (j < lines.length && !lines[j].trim()) j++;
    const next = heads[k + 1];
    if (j >= lines.length || (next && next.line === j && next.level <= h.level)) report(rel, h.line, `empty section "${h.text}"`);
  });

  let fence = false;
  lines.forEach((l, i) => {
    if (/^\s*```/.test(l)) fence = !fence;
    if (fence) return;
    const text = l.replace(/`[^`]*`/g, ""); // 코드 조각 안은 보지 않는다

    // 마크다운 링크·이미지
    const mdLink = /\[[^\]]*\]\(([^()\s]*(?:\([^()\s]*\)[^()\s]*)*)\)/g;
    for (const m of text.matchAll(mdLink)) {
      const url = m[1];
      if (checkAbsolute(rel, i, url)) continue;
      if (!/^[#./A-Za-z0-9_-]/.test(url) || /^(Alt|Ctrl|Shift|Cmd)\b/.test(url)) {
        report(rel, i, `text in parentheses right after [..] turns into a link: ${m[0]}`);
        continue;
      }
      checkTarget(rel, i, url, false);
    }

    // HTML 태그
    for (const m of text.matchAll(/<(?:img|a|source)\b[^>]*?\b(?:src|href)\s*=\s*"([^"]+)"/gi)) {
      if (!checkAbsolute(rel, i, m[1])) checkTarget(rel, i, m[1], false);
    }

    // 링크 문법 밖에 그대로 적힌 이 저장소 주소
    const bare = text.replace(mdLink, "").replace(/<[^>]*>/g, "");
    for (const m of bare.matchAll(/https:\/\/[^\s<>()"'`]+/g)) checkAbsolute(rel, i, m[0]);
  });
}

for (const [en, ko] of PAIRS) {
  const a = parse(en).heads, b = parse(ko).heads;
  if (a.length !== b.length) problems.push(`${en} vs ${ko}: ${a.length} vs ${b.length} headings`);
  for (let k = 0; k < Math.min(a.length, b.length); k++)
    if (a[k].level !== b[k].level) { problems.push(`${en}:${a[k].line + 1} "${a[k].text}" vs ${ko}:${b[k].line + 1} "${b[k].text}": heading levels differ`); break; }
}

for (const p of problems) console.log(p);
console.log(`checked ${DOCS.length} docs; problems ${problems.length}`);
process.exit(problems.length ? 1 : 0);
