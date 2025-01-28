module Bot::Webhookable
  extend ActiveSupport::Concern

  class_methods do
    def find_by_webhook(webhook)
      # TODO: wouldn't it be better to do .find_by(trigger_url: webhook) || .find_by(additional_trigger_url: webhook) ?

      queries = [{ trigger_url: webhook }.to_json, { additional_trigger_url: webhook }.to_json]
      not_deleted.find_by('settings @> ? OR settings @> ? AND settings @> \'{"additional_type_enabled":true}\'', *queries)
    end

    def generate_new_webhook_url
      loop do
        webhook = Array.new(8) { ('a'..'z').to_a.sample }.join
        break webhook unless find_by_webhook(webhook).present?
      end
    end
  end
end
