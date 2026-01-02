Rails.application.config.middleware.insert_before 0, Rack::Cors do
  allow do
    origins /localhost:\d{4}/,
            /127\.0\.0\.1:\d{4}/
  end
end
