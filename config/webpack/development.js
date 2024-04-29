process.env.NODE_ENV = process.env.NODE_ENV || "development";

const environment = require("./environment");

environment.config.merge({
//   target: "web",
  devServer: {
    liveReload: true,
    watchContentBase: true,
    contentBase: [
      "app/views",
      "app/assets",
      "public",
    ],
  },
});

module.exports = environment.toWebpackConfig();
