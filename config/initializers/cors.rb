Rails.application.config.middleware.insert_before 0, Rack::Cors do
  allow do
    origins 'https://deltabadger.com', /localhost:\d{4}/, /127\.0\.0\.1:\d{4}/,
            'deltabadger.com'
    resource '/metrics', headers: :any, methods: :get
  end
end
