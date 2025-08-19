require_relative '../../lib/ahoy_email/processor'
require_relative '../../lib/ahoy_email/database_subscriber'
require_relative '../../lib/ahoy_email/mailer'
require_relative '../../lib/ahoy_email/utils'
require_relative '../../lib/ahoy_email'

AhoyEmail.subscribers << AhoyEmail::DatabaseSubscriber
AhoyEmail.api = true
AhoyEmail.default_options[:url_options] = { host: ENV.fetch('APP_ROOT_URL') }
