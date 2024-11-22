const name = "scrpr"
const author = "elcoosp"
export default {
  language: "rs",
  title: "Scrpr",
  author,
  githubUsername: author,
  repoName: name,
  xHandle: author,
  domain: "gmail",
  email: author,
  repobeats: "60664cec3ecf2933bc6c9067f78822346f013fba",
  images: { screenshot: "images/screenshot.gif", logo: "images/logo.png" },
  sections: {
    projectDescription: "Basic rust scraper and data selector",
    about: "Scrape and select",
    installation: "sh```cargo install scrpr```",
    acknowledgments: "",
    gettingStarted: "",
    roadmap: "",
    usage: ""
  },
  template: {
    bugReport: "bug-report--from-readme",
    featRequest: "feature-request---from-readme",
  },
  backToTop: false,
} as const
