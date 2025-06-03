class Sendgrid::SyncUnsubscribesJob < ApplicationJob
  queue_as :default

  def perform
    result = client.retrieve_all_global_suppressions
    raise result.errors.to_sentence if result.failure?

    result.data.each do |suppression|
      email = suppression['email']
      user = User.find_by(email: email)
      next unless user.present? && !user.sendgrid_unsubscribed?

      result = client.get_contacts_by_emails(emails: [email])
      contact_id = Utilities::Hash.dig_or_raise(result.data, 'result', email, 'contact', 'id')
      result = client.delete_contacts(ids: [contact_id])
      raise result.errors.to_sentence if result.failure?

      user.update!(sendgrid_unsubscribed: true)
    end
  end

  private

  def client
    @client ||= SendgridClient.new
  end
end
