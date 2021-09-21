require 'json'

class HealthCheckController < ApplicationController
  def index
    render json: { health: 'check' }.to_json
  end
end
