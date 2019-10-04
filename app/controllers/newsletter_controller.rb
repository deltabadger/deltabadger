class NewsletterController < ApplicationController
  def add_email
    subscriber = Subscriber.new(email: params[:email]&.downcase)

    if subscriber.save
      render json: { data: true }, status: 201
    else
      render json: { errors: subscriber.errors.full_messages }, status: 422
    end
  end
end
