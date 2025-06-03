class Sendgrid::UpdateEmailJob < ApplicationJob
  queue_as :default

  def perform(email_was, email_now)
    result = client.get_contacts_by_emails(emails: [email_was])
    raise "No contact found for #{email_was}" if result.failure? && result.data[:status] == 404
    raise result.errors.to_sentence if result.failure?

    contact_name_was = Utilities::Hash.dig_or_raise(result.data, 'result', email_was, 'contact', 'first_name')
    contact_id_was = Utilities::Hash.dig_or_raise(result.data, 'result', email_was, 'contact', 'id')
    contact_lists_ids_was = Utilities::Hash.dig_or_raise(result.data, 'result', email_was, 'contact', 'list_ids')

    result = client.get_contacts_by_emails(emails: [email_now])
    if result.failure? && result.data[:status] == 404
      result = client.add_or_update_contacts(
        list_ids: contact_lists_ids_was,
        contacts: [
          {
            email: email_now,
            first_name: contact_name_was.presence
          }.compact
        ]
      )
      raise result.errors.to_sentence if result.failure?
    end

    return unless contact_id_was.present?

    result = client.delete_contacts(ids: [contact_id_was])
    raise result.errors.to_sentence if result.failure?
  end

  private

  def client
    @client ||= SendgridClient.new
  end
end
