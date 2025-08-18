AhoyEmail.subscribers << AhoyEmail::DatabaseSubscriber
AhoyEmail.api = true
AhoyEmail.default_options[:url_options] = { host: ENV.fetch('APP_ROOT_URL') }
