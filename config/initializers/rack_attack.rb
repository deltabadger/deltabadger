# frozen_string_literal: true

Rack::Attack.throttle('oauth/register', limit: 5, period: 60) do |req|
  req.ip if req.path == '/oauth/register' && req.post?
end

Rack::Attack.throttle('oauth/token', limit: 20, period: 60) do |req|
  req.ip if req.path == '/oauth/token' && req.post?
end

Rack::Attack.throttle('oauth/authorize', limit: 10, period: 60) do |req|
  req.ip if req.path == '/oauth/authorize' && req.get?
end

Rack::Attack.throttle('users/sign_in', limit: 10, period: 60) do |req|
  req.ip if req.path.end_with?('/users/sign_in') && req.post?
end
