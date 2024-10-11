Rails.application.config.middleware.insert_before 0, Rack::Cors do
  allow do
    origins /localhost:\d{4}/,
            /127\.0\.0\.1:\d{4}/,
            'https://deltabadger.com',
            'deltabadger.com',
            'https://legendarybadgers.com'
    resource '/metrics', headers: :any, methods: :get
  end
end
