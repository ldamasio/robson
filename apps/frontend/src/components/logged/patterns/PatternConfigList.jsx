/**
 * PatternConfigList Component
 *
 * Displays list of strategy-pattern configurations with actions.
 */

import React, { useState } from 'react';
import { Button, Table, Badge, Card, InputGroup, Form, Alert, Modal } from 'react-bootstrap';

const PatternConfigList = ({ configs, catalog, onEdit, onDelete, onCreateNew, onSelectStrategy, onRefresh }) => {
  const [searchTerm, setSearchTerm] = useState('');
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [configToDelete, setConfigToDelete] = useState(null);

  // Filter configs by search term
  const filteredConfigs = configs.filter((config) => {
    const searchLower = searchTerm.toLowerCase();
    return (
      config.strategy_name?.toLowerCase().includes(searchLower) ||
      config.pattern_name?.toLowerCase().includes(searchLower) ||
      config.pattern_code?.toLowerCase().includes(searchLower) ||
      config.symbols?.some((s) => s.toLowerCase().includes(searchLower))
    );
  });

  // Get status badge
  const getStatusBadge = (config) => {
    if (!config.is_active) return <Badge bg="secondary">Inactive</Badge>;
    if (config.auto_entry_enabled) return <Badge bg="success">Auto-Entry On</Badge>;
    return <Badge bg="warning">Suggest Only</Badge>;
  };

  // Handle delete
  const handleDeleteClick = (config) => {
    setConfigToDelete(config);
    setShowDeleteModal(true);
  };

  const confirmDelete = () => {
    if (configToDelete) {
      onDelete(configToDelete.id);
      setShowDeleteModal(false);
      setConfigToDelete(null);
    }
  };

  return (
    <div>
      {/* Header with actions */}
      <div className="d-flex justify-content-between align-items-center mb-4">
        <h4>Strategy Pattern Configurations</h4>
        <div className="d-flex gap-2">
          <Button variant="outline-primary" size="sm" onClick={onRefresh}>
            🔄 Refresh
          </Button>
          <Button variant="primary" size="sm" onClick={onCreateNew}>
            ➕ New Configuration
          </Button>
        </div>
      </div>

      {/* Search */}
      <InputGroup className="mb-4">
        <InputGroup.Text>🔍</InputGroup.Text>
        <Form.Control
          placeholder="Search by strategy, pattern, or symbol..."
          value={searchTerm}
          onChange={(e) => setSearchTerm(e.target.value)}
        />
      </InputGroup>

      {/* Empty State */}
      {configs.length === 0 ? (
        <Alert variant="info">
          <Alert.Heading>No Pattern Configurations</Alert.Heading>
          <p>
            You haven't configured any pattern detection strategies yet.
            Create a configuration to start detecting patterns.
          </p>
          <Button variant="primary" onClick={onCreateNew}>
            ➕ Create First Configuration
          </Button>
        </Alert>
      ) : (
        <Card>
          <Card.Body className="p-0">
            <Table hover responsive className="mb-0">
              <thead>
                <tr>
                  <th>Strategy</th>
                  <th>Pattern</th>
                  <th>Symbols</th>
                  <th>Timeframes</th>
                  <th>Min Confidence</th>
                  <th>Status</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {filteredConfigs.length === 0 ? (
                  <tr>
                    <td colSpan={7} className="text-center text-muted py-4">
                      No configurations match your search.
                    </td>
                  </tr>
                ) : (
                  filteredConfigs.map((config) => (
                    <tr key={config.id}>
                      <td>
                        <strong>{config.strategy_name}</strong>
                        <br />
                        <button
                          className="btn btn-link p-0 text-decoration-none"
                          style={{ fontSize: '0.75rem' }}
                          onClick={() => onSelectStrategy?.(config.strategy)}
                        >
                          View Strategy Panel →
                        </button>
                      </td>
                      <td>
                        <div>{config.pattern_name}</div>
                        <small className="text-muted">{config.pattern_code}</small>
                      </td>
                      <td>
                        {config.symbols?.length > 0 ? (
                          config.symbols.slice(0, 2).map((s, i) => (
                            <Badge key={i} bg="secondary" className="me-1">
                              {s}
                            </Badge>
                          ))
                        ) : (
                          <span className="text-muted">All</span>
                        )}
                        {config.symbols?.length > 2 && (
                          <small className="text-muted">+{config.symbols.length - 2}</small>
                        )}
                      </td>
                      <td>
                        {config.timeframes?.length > 0 ? (
                          config.timeframes.map((tf, i) => (
                            <Badge key={i} bg="info" className="me-1">
                              {tf}
                            </Badge>
                          ))
                        ) : (
                          <span className="text-muted">-</span>
                        )}
                      </td>
                      <td>
                        <Badge bg={config.min_confidence >= 0.75 ? 'success' : 'warning'}>
                          {(config.min_confidence * 100).toFixed(0)}%
                        </Badge>
                      </td>
                      <td>{getStatusBadge(config)}</td>
                      <td>
                        <div className="d-flex gap-1">
                          <Button
                            size="sm"
                            variant="outline-primary"
                            onClick={() => onEdit(config)}
                          >
                            ✏️
                          </Button>
                          <Button
                            size="sm"
                            variant="outline-danger"
                            onClick={() => handleDeleteClick(config)}
                          >
                            🗑️
                          </Button>
                        </div>
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </Table>
          </Card.Body>
        </Card>
      )}

      {/* Delete Confirmation Modal */}
      <Modal show={showDeleteModal} onHide={() => setShowDeleteModal(false)}>
        <Modal.Header closeButton>
          <Modal.Title>Confirm Delete</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          Are you sure you want to delete the pattern configuration for{' '}
          <strong>
            {configToDelete?.strategy_name} - {configToDelete?.pattern_name}
          </strong>
          ?
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setShowDeleteModal(false)}>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirmDelete}>
            Delete
          </Button>
        </Modal.Footer>
      </Modal>
    </div>
  );
};

export default PatternConfigList;
