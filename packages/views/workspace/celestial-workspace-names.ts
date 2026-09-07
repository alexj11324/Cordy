import type { SupportedLocale } from "@patchbay/core/i18n";

export interface CelestialWorkspaceName {
  slugBase: string;
  names: Record<SupportedLocale, string>;
}

export const CELESTIAL_WORKSPACE_NAMES = [
  {
    slugBase: "alpha-centauri",
    names: {
      en: "Alpha Centauri",
      "zh-Hans": "南门二",
    },
  },
  {
    slugBase: "andromeda",
    names: {
      en: "Andromeda",
      "zh-Hans": "仙女座星系",
    },
  },
  {
    slugBase: "antares",
    names: {
      en: "Antares",
      "zh-Hans": "心宿二",
    },
  },
  {
    slugBase: "ariel",
    names: {
      en: "Ariel",
      "zh-Hans": "天卫一",
    },
  },
  {
    slugBase: "betelgeuse",
    names: {
      en: "Betelgeuse",
      "zh-Hans": "参宿四",
    },
  },
  {
    slugBase: "callisto",
    names: {
      en: "Callisto",
      "zh-Hans": "木卫四",
    },
  },
  {
    slugBase: "capella",
    names: {
      en: "Capella",
      "zh-Hans": "五车二",
    },
  },
  {
    slugBase: "ceres",
    names: {
      en: "Ceres",
      "zh-Hans": "谷神星",
    },
  },
  {
    slugBase: "deimos",
    names: {
      en: "Deimos",
      "zh-Hans": "火卫二",
    },
  },
  {
    slugBase: "deneb",
    names: {
      en: "Deneb",
      "zh-Hans": "天津四",
    },
  },
  {
    slugBase: "dione",
    names: {
      en: "Dione",
      "zh-Hans": "土卫四",
    },
  },
  {
    slugBase: "enceladus",
    names: {
      en: "Enceladus",
      "zh-Hans": "土卫二",
    },
  },
  {
    slugBase: "eris",
    names: {
      en: "Eris",
      "zh-Hans": "阋神星",
    },
  },
  {
    slugBase: "europa",
    names: {
      en: "Europa",
      "zh-Hans": "木卫二",
    },
  },
  {
    slugBase: "ganymede",
    names: {
      en: "Ganymede",
      "zh-Hans": "木卫三",
    },
  },
  {
    slugBase: "halley",
    names: {
      en: "Halley",
      "zh-Hans": "哈雷彗星",
    },
  },
  {
    slugBase: "hyperion",
    names: {
      en: "Hyperion",
      "zh-Hans": "土卫七",
    },
  },
  {
    slugBase: "io",
    names: {
      en: "Io",
      "zh-Hans": "木卫一",
    },
  },
  {
    slugBase: "mars",
    names: {
      en: "Mars",
      "zh-Hans": "火星",
    },
  },
  {
    slugBase: "mercury",
    names: {
      en: "Mercury",
      "zh-Hans": "水星",
    },
  },
  {
    slugBase: "mimas",
    names: {
      en: "Mimas",
      "zh-Hans": "土卫一",
    },
  },
  {
    slugBase: "miranda",
    names: {
      en: "Miranda",
      "zh-Hans": "天卫五",
    },
  },
  {
    slugBase: "neptune",
    names: {
      en: "Neptune",
      "zh-Hans": "海王星",
    },
  },
  {
    slugBase: "oberon",
    names: {
      en: "Oberon",
      "zh-Hans": "天卫四",
    },
  },
  {
    slugBase: "orion-nebula",
    names: {
      en: "Orion Nebula",
      "zh-Hans": "猎户座星云",
    },
  },
  {
    slugBase: "phobos",
    names: {
      en: "Phobos",
      "zh-Hans": "火卫一",
    },
  },
  {
    slugBase: "pluto",
    names: {
      en: "Pluto",
      "zh-Hans": "冥王星",
    },
  },
  {
    slugBase: "polaris",
    names: {
      en: "Polaris",
      "zh-Hans": "北极星",
    },
  },
  {
    slugBase: "proxima-centauri",
    names: {
      en: "Proxima Centauri",
      "zh-Hans": "比邻星",
    },
  },
  {
    slugBase: "rhea",
    names: {
      en: "Rhea",
      "zh-Hans": "土卫五",
    },
  },
  {
    slugBase: "rigel",
    names: {
      en: "Rigel",
      "zh-Hans": "参宿七",
    },
  },
  {
    slugBase: "saturn",
    names: {
      en: "Saturn",
      "zh-Hans": "土星",
    },
  },
  {
    slugBase: "sirius",
    names: {
      en: "Sirius",
      "zh-Hans": "天狼星",
    },
  },
  {
    slugBase: "sombrero-galaxy",
    names: {
      en: "Sombrero Galaxy",
      "zh-Hans": "草帽星系",
    },
  },
  {
    slugBase: "titan",
    names: {
      en: "Titan",
      "zh-Hans": "土卫六",
    },
  },
  {
    slugBase: "titania",
    names: {
      en: "Titania",
      "zh-Hans": "天卫三",
    },
  },
  {
    slugBase: "triton",
    names: {
      en: "Triton",
      "zh-Hans": "海卫一",
    },
  },
  {
    slugBase: "vega",
    names: {
      en: "Vega",
      "zh-Hans": "织女星",
    },
  },
  {
    slugBase: "venus",
    names: {
      en: "Venus",
      "zh-Hans": "金星",
    },
  },
  {
    slugBase: "vesta",
    names: {
      en: "Vesta",
      "zh-Hans": "灶神星",
    },
  },
  {
    slugBase: "achernar",
    names: {
      en: "Achernar",
      "zh-Hans": "水委一",
    },
  },
  {
    slugBase: "acrux",
    names: {
      en: "Acrux",
      "zh-Hans": "十字架二",
    },
  },
  {
    slugBase: "adhara",
    names: {
      en: "Adhara",
      "zh-Hans": "弧矢七",
    },
  },
  {
    slugBase: "adrastea",
    names: {
      en: "Adrastea",
      "zh-Hans": "木卫十五",
    },
  },
  {
    slugBase: "alcyone",
    names: {
      en: "Alcyone",
      "zh-Hans": "昴宿六",
    },
  },
  {
    slugBase: "aldebaran",
    names: {
      en: "Aldebaran",
      "zh-Hans": "毕宿五",
    },
  },
  {
    slugBase: "algol",
    names: {
      en: "Algol",
      "zh-Hans": "大陵五",
    },
  },
  {
    slugBase: "alhena",
    names: {
      en: "Alhena",
      "zh-Hans": "井宿三",
    },
  },
  {
    slugBase: "alnair",
    names: {
      en: "Alnair",
      "zh-Hans": "鹤一",
    },
  },
  {
    slugBase: "alnilam",
    names: {
      en: "Alnilam",
      "zh-Hans": "参宿二",
    },
  },
  {
    slugBase: "alnitak",
    names: {
      en: "Alnitak",
      "zh-Hans": "参宿一",
    },
  },
  {
    slugBase: "altair",
    names: {
      en: "Altair",
      "zh-Hans": "牛郎星",
    },
  },
  {
    slugBase: "amalthea",
    names: {
      en: "Amalthea",
      "zh-Hans": "木卫五",
    },
  },
  {
    slugBase: "ananke",
    names: {
      en: "Ananke",
      "zh-Hans": "木卫十二",
    },
  },
  {
    slugBase: "arcturus",
    names: {
      en: "Arcturus",
      "zh-Hans": "大角星",
    },
  },
  {
    slugBase: "bellatrix",
    names: {
      en: "Bellatrix",
      "zh-Hans": "参宿五",
    },
  },
  {
    slugBase: "bianca",
    names: {
      en: "Bianca",
      "zh-Hans": "天卫八",
    },
  },
  {
    slugBase: "canopus",
    names: {
      en: "Canopus",
      "zh-Hans": "老人星",
    },
  },
  {
    slugBase: "carme",
    names: {
      en: "Carme",
      "zh-Hans": "木卫十一",
    },
  },
  {
    slugBase: "cartwheel-galaxy",
    names: {
      en: "Cartwheel Galaxy",
      "zh-Hans": "车轮星系",
    },
  },
  {
    slugBase: "castor",
    names: {
      en: "Castor",
      "zh-Hans": "北河二",
    },
  },
  {
    slugBase: "charon",
    names: {
      en: "Charon",
      "zh-Hans": "冥卫一",
    },
  },
  {
    slugBase: "cordelia",
    names: {
      en: "Cordelia",
      "zh-Hans": "天卫六",
    },
  },
  {
    slugBase: "crab-nebula",
    names: {
      en: "Crab Nebula",
      "zh-Hans": "蟹状星云",
    },
  },
  {
    slugBase: "cygnus-x-1",
    names: {
      en: "Cygnus X-1",
      "zh-Hans": "天鹅座 X-1",
    },
  },
  {
    slugBase: "despina",
    names: {
      en: "Despina",
      "zh-Hans": "海卫五",
    },
  },
  {
    slugBase: "elara",
    names: {
      en: "Elara",
      "zh-Hans": "木卫七",
    },
  },
  {
    slugBase: "electra",
    names: {
      en: "Electra",
      "zh-Hans": "昴宿一",
    },
  },
  {
    slugBase: "fomalhaut",
    names: {
      en: "Fomalhaut",
      "zh-Hans": "北落师门",
    },
  },
  {
    slugBase: "haumea",
    names: {
      en: "Haumea",
      "zh-Hans": "妊神星",
    },
  },
  {
    slugBase: "helene",
    names: {
      en: "Helene",
      "zh-Hans": "土卫十二",
    },
  },
  {
    slugBase: "iapetus",
    names: {
      en: "Iapetus",
      "zh-Hans": "土卫八",
    },
  },
  {
    slugBase: "janus",
    names: {
      en: "Janus",
      "zh-Hans": "土卫十",
    },
  },
  {
    slugBase: "juliet",
    names: {
      en: "Juliet",
      "zh-Hans": "天卫十一",
    },
  },
  {
    slugBase: "larissa",
    names: {
      en: "Larissa",
      "zh-Hans": "海卫七",
    },
  },
  {
    slugBase: "leda",
    names: {
      en: "Leda",
      "zh-Hans": "木卫十三",
    },
  },
  {
    slugBase: "makemake",
    names: {
      en: "Makemake",
      "zh-Hans": "鸟神星",
    },
  },
  {
    slugBase: "merope",
    names: {
      en: "Merope",
      "zh-Hans": "昴宿五",
    },
  },
  {
    slugBase: "metis",
    names: {
      en: "Metis",
      "zh-Hans": "木卫十六",
    },
  },
  {
    slugBase: "mintaka",
    names: {
      en: "Mintaka",
      "zh-Hans": "参宿三",
    },
  },
  {
    slugBase: "naiad",
    names: {
      en: "Naiad",
      "zh-Hans": "海卫三",
    },
  },
  {
    slugBase: "nereid",
    names: {
      en: "Nereid",
      "zh-Hans": "海卫二",
    },
  },
  {
    slugBase: "ophelia",
    names: {
      en: "Ophelia",
      "zh-Hans": "天卫七",
    },
  },
  {
    slugBase: "pan",
    names: {
      en: "Pan",
      "zh-Hans": "土卫十八",
    },
  },
  {
    slugBase: "pandora",
    names: {
      en: "Pandora",
      "zh-Hans": "土卫十七",
    },
  },
  {
    slugBase: "pasiphae",
    names: {
      en: "Pasiphae",
      "zh-Hans": "木卫八",
    },
  },
  {
    slugBase: "phoebe",
    names: {
      en: "Phoebe",
      "zh-Hans": "土卫九",
    },
  },
  {
    slugBase: "pinwheel-galaxy",
    names: {
      en: "Pinwheel Galaxy",
      "zh-Hans": "风车星系",
    },
  },
  {
    slugBase: "pollux",
    names: {
      en: "Pollux",
      "zh-Hans": "北河三",
    },
  },
  {
    slugBase: "portia",
    names: {
      en: "Portia",
      "zh-Hans": "天卫十二",
    },
  },
  {
    slugBase: "proteus",
    names: {
      en: "Proteus",
      "zh-Hans": "海卫八",
    },
  },
  {
    slugBase: "puck",
    names: {
      en: "Puck",
      "zh-Hans": "天卫十五",
    },
  },
  {
    slugBase: "regulus",
    names: {
      en: "Regulus",
      "zh-Hans": "轩辕十四",
    },
  },
  {
    slugBase: "rosalind",
    names: {
      en: "Rosalind",
      "zh-Hans": "天卫十三",
    },
  },
  {
    slugBase: "spica",
    names: {
      en: "Spica",
      "zh-Hans": "角宿一",
    },
  },
  {
    slugBase: "sycorax",
    names: {
      en: "Sycorax",
      "zh-Hans": "天卫十七",
    },
  },
  {
    slugBase: "telesto",
    names: {
      en: "Telesto",
      "zh-Hans": "土卫十三",
    },
  },
  {
    slugBase: "thebe",
    names: {
      en: "Thebe",
      "zh-Hans": "木卫十四",
    },
  },
  {
    slugBase: "umbriel",
    names: {
      en: "Umbriel",
      "zh-Hans": "天卫二",
    },
  },
  {
    slugBase: "whirlpool-galaxy",
    names: {
      en: "Whirlpool Galaxy",
      "zh-Hans": "涡状星系",
    },
  },
] as const satisfies readonly CelestialWorkspaceName[];
