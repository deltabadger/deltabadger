const { environment } = require('@rails/webpacker')
const erb =  require('./loaders/erb')

environment.loaders.get('sass').use.splice(-1, 0, {
  loader: 'resolve-url-loader',
  options: {
    attempts: 1
  }
});

environment.loaders.prepend('erb', erb)
module.exports = environment
