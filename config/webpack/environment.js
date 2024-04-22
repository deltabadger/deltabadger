const { environment } = require('@rails/webpacker')

environment.config.merge({
  module: {
    rules: [
      {
        test: /\.js$/,
        include: /node_modules/,
        use: [{
          loader: 'babel-loader',
          options: {
            plugins: [
              '@babel/plugin-transform-optional-chaining',
            ],
          },
        }]
      },
      {
        test: /\.mjs$/,
        include: /node_modules/,
        type: "javascript/auto",
        use: [{
          loader: 'babel-loader',
        }]
      },
      {
        test: /\.erb$/,
        enforce: 'pre',
        exclude: /node_modules/,
        use: [{
          loader: 'rails-erb-loader',
          options: {
            runner: (/^win/.test(process.platform) ? 'ruby ' : '') + 'bin/rails runner'
          }
        }]
      }
    ],
  },
});

module.exports = environment
