module ApplicationHelper
  include Pagy::Frontend

  EXCHANGE_SVGS = Hash.new do |hash, name_id|
    path = Rails.root.join("app/views/svg/_exchange-#{name_id}.html.erb")
    hash[name_id] = File.read(path).html_safe.freeze
  end

  def exchange_icon_svg(exchange_name_id)
    EXCHANGE_SVGS[exchange_name_id]
  end
end
