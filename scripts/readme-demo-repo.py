#!/usr/bin/env python3
"""The demo repository of the README's images (scripts/readme-images.sh):
scripts/readme-demo-repo.py WORK [--rebasing] makes WORK/storefront, with worktrees beside it, an
origin on "GitHub" (acme/storefront), its open pull requests in WORK/prs.json, and a branch
rebased but not pushed. With --rebasing, a worktree is stopped part-way through a rebase.
WORK is deleted first."""
import os, shutil, subprocess, sys

work = os.path.abspath(sys.argv[1])
if work in ("/", os.path.expanduser("~")):
    sys.exit(f"refusing to delete {work}")
shutil.rmtree(work, ignore_errors=True)
os.makedirs(work)
repo = f"{work}/storefront"
origin = f"{work}/origin.git"
t = [1767000000]
AUTHORS = {
    "mira": ("Mira Lindqvist", "mira@acme.dev"),
    "jonas": ("Jonas Berg", "jonas@acme.dev"),
    "sam": ("Sam Okafor", "sam@acme.dev"),
    "lena": ("Lena Park", "lena@acme.dev"),
    "bot": ("dependabot[bot]", "bot@acme.dev"),
}

def git(*args, cwd=repo, **kw):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True, **kw).stdout.strip()

def write(path, text, cwd=None):
    full = os.path.join(cwd or repo, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text.lstrip("\n"))

def env(who):
    t[0] += 3 * 3600 + 17 * 60
    name, mail = AUTHORS[who]
    stamp = f"{t[0]} +0100"
    return {**os.environ, "GIT_AUTHOR_NAME": name, "GIT_AUTHOR_EMAIL": mail, "GIT_COMMITTER_NAME": name,
            "GIT_COMMITTER_EMAIL": mail, "GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp}

def commit(who, msg, files, cwd=None):
    for path, text in files.items():
        write(path, text, cwd)
    git("add", "-A", cwd=cwd or repo)
    git("commit", "-q", "-m", msg, cwd=cwd or repo, env=env(who))

def merge(who, branch, msg):
    git("merge", "-q", "--no-ff", "-m", msg, branch, env=env(who))

def tag(name, msg):
    git("tag", "-a", name, "-m", msg, env=env("lena"))

os.makedirs(repo)
git("init", "-q", "-b", "main")

CART = """
//! The shopping cart.

use crate::catalogue::Product;

pub struct Line {
    pub product: Product,
    pub quantity: u32,
}

#[derive(Default)]
pub struct Cart {
    pub lines: Vec<Line>,
}

impl Cart {
    pub fn add(&mut self, product: Product, quantity: u32) {
        match self.lines.iter_mut().find(|l| l.product.sku == product.sku) {
            Some(line) => line.quantity += quantity,
            None => self.lines.push(Line { product, quantity }),
        }
    }

    pub fn remove(&mut self, sku: &str) {
        self.lines.retain(|l| l.product.sku != sku);
    }
%TOTAL%}
"""
TOTAL_FLOAT = """
    /// The total, in euros.
    pub fn total(&self) -> f64 {
        self.lines
            .iter()
            .map(|l| l.product.price * l.quantity as f64)
            .sum()
    }
"""
TOTAL_CENTS = """
    /// The total, in cents, so that it never rounds.
    pub fn total(&self) -> u64 {
        self.lines
            .iter()
            .map(|l| l.product.price_cents * u64::from(l.quantity))
            .sum()
    }
"""
CATALOGUE = """
//! Products and the catalogue.

#[derive(Clone, Debug)]
pub struct Product {
    pub sku: String,
    pub name: String,
    pub %PRICE%,
}

pub struct Catalogue {
    products: Vec<Product>,
}

impl Catalogue {
    pub fn load(path: &str) -> std::io::Result<Catalogue> {
        let text = std::fs::read_to_string(path)?;
        Ok(Catalogue { products: parse(&text) })
    }
%SEARCH%}
"""
SEARCH = """
    /// Products whose name contains `query`, case-insensitively.
    pub fn search(&self, query: &str) -> Vec<&Product> {
        let query = query.to_lowercase();
        self.products
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&query))
            .collect()
    }
"""
README = """
# Storefront

A small web shop: catalogue, cart and checkout.

## Running

    cargo run -- --catalogue products.csv
%MORE%"""
CHECKOUT = """
//! Checkout: from cart to order.

use crate::cart::Cart;
use crate::payment::Provider;

pub struct Order {
    pub id: u64,
    pub total: u64,
}

pub fn checkout(cart: &Cart, payment: &dyn Provider) -> Result<Order, String> {
    if cart.lines.is_empty() {
        return Err("the cart is empty".into());
    }
%STEPS%    let total = cart.total();
    payment.charge(total)?;
    Ok(Order { id: next_id(), total })
}
"""
def cart(total=TOTAL_FLOAT): return CART.replace("%TOTAL%", total)
def catalogue(price="price: f64", search=""): return CATALOGUE.replace("%PRICE%", price).replace("%SEARCH%", search)
def checkout(steps=""): return CHECKOUT.replace("%STEPS%", steps)

commit("lena", "Initial commit", {"README.md": README.replace("%MORE%", ""), "Cargo.toml": '[package]\nname = "storefront"\nversion = "0.1.0"\nedition = "2021"\n'})
commit("lena", "Product catalogue", {"src/catalogue.rs": catalogue(), "src/main.rs": "mod catalogue;\n\nfn main() {}\n"})
tag("v1.0.0", "First release")
git("switch", "-q", "-c", "feature/cart")
commit("jonas", "Shopping cart", {"src/cart.rs": cart("")})
commit("jonas", "Cart totals", {"src/cart.rs": cart()})
git("switch", "-q", "main")
commit("mira", "Search products", {"src/catalogue.rs": catalogue(search=SEARCH)})
merge("lena", "feature/cart", "Merge pull request #12 from acme/feature/cart")
tag("v1.1.0", "Cart and search")
git("branch", "-q", "-d", "feature/cart")
git("switch", "-q", "-c", "release/1.1")
commit("jonas", "Fix crash on an empty search", {"src/catalogue.rs": catalogue(search=SEARCH.replace("let query = query", "if query.is_empty() {\n            return Vec::new();\n        }\n        let query = query"))})
tag("v1.1.1", "Hotfix")
git("switch", "-q", "main")
commit("sam", "Payment provider abstraction", {"src/payment.rs": "pub trait Provider {\n    fn charge(&self, cents: u64) -> Result<(), String>;\n}\n"})
merge("lena", "release/1.1", "Merge release/1.1 fixes")
commit("bot", "Bump serde from 1.0.210 to 1.0.214", {"Cargo.lock": "serde 1.0.214\n"})
tag("v1.2.0", "Payments")
base_checkout = git("rev-parse", "HEAD")
git("switch", "-q", "-c", "fix/cart-rounding")
commit("jonas", "Cart: keep prices in cents", {"src/catalogue.rs": catalogue("price_cents: u64", SEARCH)})
commit("jonas", "Cart: total in cents", {"src/cart.rs": cart(TOTAL_CENTS)})
git("switch", "-q", "main")
commit("mira", "Order history", {"src/orders.rs": "//! Past orders of a customer.\n\npub struct History {\n    pub orders: Vec<u64>,\n}\n"})
git("switch", "-q", "-c", "feature/dark-mode")
commit("sam", "Dark palette", {"assets/theme.css": ":root { --bg: #fff; --fg: #222; }\n@media (prefers-color-scheme: dark) {\n  :root { --bg: #16181d; --fg: #e6e6e6; }\n}\n"})
commit("sam", "Theme switcher", {"assets/theme.js": "export function toggleTheme() {\n  document.body.classList.toggle('dark');\n}\n"})
git("switch", "-q", "main")
git("switch", "-q", "-c", "feature/checkout-redesign", base_checkout)
commit("mira", "Checkout: one-page layout", {"src/checkout.rs": checkout()})
commit("mira", "Checkout: address autocomplete", {"src/checkout.rs": checkout("    let address = autocomplete(&cart.address)?;\n")})
git("switch", "-q", "main")
commit("lena", "Faster product images", {"README.md": README.replace("%MORE%", "\nImages are resized once, at upload.\n")})
git("switch", "-q", "-c", "dependabot/cargo/tokio-1.41.1")
commit("bot", "Bump tokio from 1.40.0 to 1.41.1", {"Cargo.lock": "serde 1.0.214\ntokio 1.41.1\n"})
git("switch", "-q", "main")
git("switch", "-q", "-c", "experiment/wasm")
commit("sam", "Prototype: catalogue in WebAssembly", {"wasm/lib.rs": "// Runs the catalogue search in the browser.\n"})
git("switch", "-q", "main")

# Publish everything, then work on: origin is "GitHub" from here on.
git("clone", "-q", "--bare", repo, origin, cwd=work)
git("remote", "add", "origin", origin)
git("fetch", "-q", "origin")
for b in ["main", "feature/checkout-redesign", "fix/cart-rounding", "feature/dark-mode"]:
    git("branch", "-q", "-u", f"origin/{b}", b)
git("push", "-q", "origin", ":experiment/wasm")
git("fetch", "-q", "--prune", "origin")
# main: someone merged on GitHub; not pulled yet (main 0|1).
other = f"{work}/other"
git("clone", "-q", origin, other, cwd=work)
git("switch", "-q", "-c", "fix/typo", cwd=other)
commit("lena", "README: fix a typo", {"README.md": README.replace("%MORE%", "\nImages are resized once, on upload.\n")}, cwd=other)
git("switch", "-q", "main", cwd=other)
git("merge", "-q", "--no-ff", "-m", "Merge pull request #144 from acme/fix/typo", "fix/typo", cwd=other, env=env("lena"))
git("push", "-q", "origin", "main", cwd=other)
git("fetch", "-q", "origin")
# fix/cart-rounding: rebased onto main here, not pushed yet; a dashed edge to its upstream.
git("switch", "-q", "fix/cart-rounding")
git("rebase", "-q", "main", env=env("jonas"))
commit("jonas", "Cart: round only when showing prices", {"src/cart.rs": cart(TOTAL_CENTS + "\n    pub fn display_total(&self) -> String {\n        format!(\"{:.2} EUR\", self.total() as f64 / 100.0)\n    }\n")})
git("switch", "-q", "main")
# feature/checkout-redesign: in a worktree of its own, with work in progress.
wt = f"{work}/storefront-checkout"
git("worktree", "add", "-q", wt, "feature/checkout-redesign")
commit("mira", "Checkout: review step", {"src/checkout.rs": checkout("    let address = autocomplete(&cart.address)?;\n    review(cart, &address)?;\n")}, cwd=wt)
if "--rebasing" in sys.argv:
    # Stopped part-way through a rebase onto main: an orange zigzag to the branch.
    subprocess.run(["git", "rebase", "-i", "main"], cwd=wt, check=True, capture_output=True,
                   env={**env("mira"), "GIT_SEQUENCE_EDITOR": "sed -i '2a break'"})
else:
  write("src/checkout.rs", checkout("    let address = autocomplete(&cart.address)?;\n    review(cart, &address)?;\n    log::info!(\"checkout of {} lines\", cart.lines.len());\n"), cwd=wt)
# A detached worktree, reviewing the dark mode pull request.
git("worktree", "add", "-q", "--detach", f"{work}/review-139", "origin/feature/dark-mode")
git("remote", "set-url", "origin", "https://github.com/acme/storefront.git")

with open(f"{work}/prs.json", "w") as f:
    f.write("""[
  {"number": 148, "title": "Bump tokio from 1.40.0 to 1.41.1", "author": "dependabot", "head": "dependabot/cargo/tokio-1.41.1", "base": "main"},
  {"number": 146, "title": "Checkout redesign", "author": "mira", "head": "feature/checkout-redesign", "base": "main"},
  {"number": 145, "title": "Keep cart totals in cents", "author": "jonas", "head": "fix/cart-rounding", "base": "main"},
  {"number": 139, "title": "Dark mode", "author": "sam", "draft": true, "head": "feature/dark-mode", "base": "main"}
]
""")
print(repo)
