Rails.application.config.middleware.insert_before 0, Rack::Cors do
  allow do
    origins 'https://deltabadger.com'
    resource '/metrics', headers: :any, methods: :get
  end
end
