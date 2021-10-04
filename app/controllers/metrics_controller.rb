require 'json'
class MetricsController < ApplicationController
  def index
    render json: { data: MetricsRepository.new.metrics_data }.to_json
  end
end
