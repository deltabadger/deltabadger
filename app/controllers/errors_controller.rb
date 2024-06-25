class ErrorsController < ApplicationController
  def not_found
    render file: "#{Rails.root}/public/404.html", status: :not_found, layout: false
  end

  def internal_server_error
    render file: "#{Rails.root}/public/500.html", status: :internal_server_error, layout: false
  end

  def unprocessable_entity
    render file: "#{Rails.root}/public/422.html", status: :unprocessable_entity, layout: false
  end
end
